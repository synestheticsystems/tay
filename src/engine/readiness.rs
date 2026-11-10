//! A unit's readiness check, made once: an `http` GET that gets any 2xx
//! answer, a `tcp` connection that opens, both made from the host; a
//! `command` that exits 0, run where the unit runs; or a line the unit wrote
//! that contains the text.

use std::sync::Arc;
use std::time::Duration;

use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::client::conn::http1;
use hyper::{Request, Uri};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::clock;
use crate::definition::Check;
use crate::module::{Running, UnitPlace};
use crate::unit_log::{Following, Line, UnitLog, Writer};
use crate::value::Filled;

/// A readiness check, with its values filled.
pub enum Probe {
    Http(String),
    Tcp(String),
    Command(Vec<String>),
    Log {
        text: String,
        log: UnitLog,
        /// The time the attempt's program started, as the log writes times:
        /// a line written before it is no attempt's of this one.
        since: String,
        /// The log, read as far as it has been looked at.
        following: Option<Following>,
    },
}

impl Probe {
    /// The check `check`, filled, of a unit whose log is `log` and whose
    /// program started at `since`, in milliseconds since 1970. A readiness
    /// check holds no secret, since the planner refuses one there, so its
    /// values are said as they are.
    pub fn of(check: &Check<Filled>, log: UnitLog, since: u64) -> Probe {
        match check {
            Check::Http(url) => Probe::Http(url.reveal()),
            Check::Tcp(address) => Probe::Tcp(address.reveal()),
            Check::Command(words) => Probe::Command(words.iter().map(Filled::reveal).collect()),
            Check::Log(text) => Probe::Log {
                text: text.reveal(),
                log,
                since: clock::stamp(since),
                following: None,
            },
        }
    }

    /// Why `check` can never pass, whatever the unit does: an `http` check
    /// of a URL tay cannot ask.
    pub fn impossible(check: &Check<Filled>) -> Option<String> {
        let Check::Http(url) = check else {
            return None;
        };
        let url = url.reveal();
        match url.parse::<Uri>().ok()?.scheme_str() {
            Some("http") => None,
            Some("https") => Some(format!(
                "its check `http {url}` is of an `https` address, and tay cannot make an `https` check yet"
            )),
            _ => Some(format!("its check `http {url}` is not of an `http` URL")),
        }
    }

    /// The check as `ready` writes it.
    pub fn words(&self) -> String {
        match self {
            Probe::Http(url) => format!("http {url}"),
            Probe::Tcp(address) => format!("tcp {address}"),
            Probe::Command(words) => format!("command `{}`", words.join(" ")),
            Probe::Log { text, .. } => format!("log \"{text}\""),
        }
    }

    /// Whether a check that has passed stays passed for as long as the
    /// attempt runs: a line once written stays written.
    pub fn stays_passed(&self) -> bool {
        matches!(self, Probe::Log { .. })
    }

    /// Makes the check once, for as long as `limit`; why it did not pass.
    pub async fn once(
        &mut self,
        running: &Arc<dyn Running>,
        place: &UnitPlace,
        limit: Duration,
    ) -> Result<(), String> {
        match self {
            Probe::Http(url) => within(limit, get(url)).await,
            Probe::Tcp(address) => within(limit, connect(address)).await,
            Probe::Command(words) => {
                Arc::clone(running)
                    .check(place.clone(), words.clone(), limit)
                    .await
            }
            Probe::Log {
                text,
                log,
                since,
                following,
            } => {
                let mut lines = Vec::new();
                match following {
                    Some(following) => following.read(since, &mut lines),
                    None => log.read_since(since).map(|(read, rest)| {
                        lines = read;
                        *following = Some(rest);
                    }),
                }
                .map_err(|error| error.to_string())?;
                let written =
                    |line: &Line| line.writer == Writer::Unit && line.text.contains(&*text);
                match lines.iter().any(written) {
                    true => Ok(()),
                    false => Err("no line it wrote has it yet".to_string()),
                }
            }
        }
    }
}

async fn within(
    limit: Duration,
    check: impl std::future::Future<Output = Result<(), String>>,
) -> Result<(), String> {
    tokio::time::timeout(limit, check)
        .await
        .unwrap_or_else(|_| Err(format!("no answer within {} seconds", limit.as_secs())))
}

async fn connect(address: &str) -> Result<(), String> {
    TcpStream::connect(address)
        .await
        .map(drop)
        .map_err(|error| format!("no connection opens: {error}"))
}

/// A GET of `url`, which passes with any 2xx answer.
async fn get(url: &str) -> Result<(), String> {
    let uri: Uri = url
        .parse()
        .map_err(|_| format!("`{url}` is not a URL tay can ask"))?;
    match uri.scheme_str() {
        Some("http") => {}
        Some("https") => return Err("tay cannot make an `https` check yet".to_string()),
        _ => return Err(format!("`{url}` is not an `http` URL")),
    }
    let authority = uri
        .authority()
        .ok_or_else(|| format!("`{url}` names no host"))?
        .clone();
    let port = authority.port_u16().unwrap_or(80);
    let host = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let stream = TcpStream::connect((host, port))
        .await
        .map_err(|error| format!("no connection opens: {error}"))?;
    let broken = |error: hyper::Error| format!("the answer did not come: {error}");
    let (mut sender, connection) = http1::handshake(TokioIo::new(stream))
        .await
        .map_err(broken)?;
    // The connection is driven apart from the request, and ends with it.
    tokio::spawn(connection);
    let path = uri.path_and_query().map_or("/", |path| path.as_str());
    let request = Request::get(path)
        .header("host", authority.as_str())
        .body(Empty::<Bytes>::new())
        .map_err(|error| format!("`{url}` cannot be asked: {error}"))?;
    let response = sender.send_request(request).await.map_err(broken)?;
    match response.status() {
        status if status.is_success() => Ok(()),
        status => Err(format!("it answered {status}")),
    }
}
