//! The commands' side of an environment's daemon: finding the daemon,
//! starting one when none answers, replacing one of an older tay, and asking
//! it for what each command does.
//!
//! A socket file alone proves nothing: only a daemon that answers on it as
//! the daemon of this environment does. Whether a daemon runs is decided by
//! its lock, never by a file holding its process's number.

mod http;
mod start;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use hyper::{Method, StatusCode};
use rustix::io::Errno;
use serde::de::DeserializeOwned;

use crate::control::protocol::{self, Event, Problem, Refused, Status, UpRequest};
use crate::definition::read::{THIS_TAY, parse_version};
use crate::disk::{DiskError, Environment, Lock, socket_fits};

use http::NoAnswer;
use start::{NotStarted, START, Started};

/// How often a command looks again for a daemon that is starting or
/// ending.
const RETRY: Duration = Duration::from_millis(50);

/// A daemon that answers for an environment.
pub struct Client {
    socket: PathBuf,
    pub daemon: protocol::About,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Another change is under way.
    #[error("another change is running: `{}`, since {}", .0.change, crate::clock::utc(.0.since))]
    Refused(Refused),
    #[error("the daemon cannot be started: {0}")]
    NotStarted(#[from] NotStarted),
    #[error("the daemon cannot be reached: {0}")]
    NoAnswer(#[from] NoAnswer),
    #[error("no daemon answered within {} seconds; see `{}`", START.as_secs(), .0.display())]
    NoDaemon(PathBuf),
    #[error("the daemon answered in a way this tay does not read: {0}")]
    Garbled(String),
    #[error("the socket `{}` answers for the environment at `{}`, not this one", socket.display(), root.display())]
    Other { socket: PathBuf, root: PathBuf },
    #[error(
        "the daemon is of tay {theirs}, and this tay is {THIS_TAY}: run this command with a tay of {theirs}"
    )]
    Newer { theirs: String },
    #[error("the daemon (process {pid}) said it would exit and has not within {} seconds", START.as_secs())]
    StillRunning { pid: u32 },
    #[error("the daemon went away before it had finished")]
    WentAway,
    #[error("the daemon began to exit as this command reached it; run the command again")]
    Exiting,
    #[error(transparent)]
    Disk(#[from] DiskError),
}

impl Client {
    /// The daemon that answers for `environment`, if one does.
    pub async fn find(environment: &Environment) -> Result<Option<Client>, ClientError> {
        let socket = environment.socket();
        // Nothing can listen on a socket whose path is too long to have.
        if !socket_fits(&socket) {
            return Ok(None);
        }
        let (status, body) = match http::ask(
            &socket,
            Method::GET,
            protocol::DAEMON,
            Vec::new(),
            http::WHO,
        )
        .await
        {
            Ok((StatusCode::SERVICE_UNAVAILABLE, _)) | Err(NoAnswer::NotListening(_)) => {
                return Ok(None);
            }
            Ok(answer) => answer,
            Err(no_answer) => return Err(no_answer.into()),
        };
        let daemon: protocol::About = parse(status, &body)?;
        if daemon.root != environment.layout.root() || daemon.id != environment.id.as_str() {
            return Err(ClientError::Other {
                socket,
                root: daemon.root,
            });
        }
        Ok(Some(Client { socket, daemon }))
    }

    /// The daemon of `environment`, started if none answers, and started
    /// again when the one that answers is of an older tay; the older one is
    /// asked to exit, which leaves the stack as it is. A daemon of a newer
    /// tay is left alone.
    pub async fn ensure(environment: &Environment) -> Result<Client, ClientError> {
        environment
            .machine
            .make_socket_folder(environment.id.as_str())?;
        if let Some(client) = Client::find(environment).await?
            && let Some(client) = client.of_this_tay().await?
        {
            return Ok(client);
        }
        // One command starts the daemon: the one that holds this lock.
        let bootstrap = environment.bootstrap_lock();
        let _bootstrap = tokio::task::spawn_blocking(move || Lock::take(&bootstrap))
            .await
            .expect("waiting for a lock does not panic")?;
        let until = Instant::now() + START;
        let mut started = false;
        loop {
            if let Some(client) = Client::find(environment).await?
                && let Some(client) = client.of_this_tay().await?
            {
                return Ok(client);
            }
            // Holding the daemon's lock proves that no daemon runs, so a
            // socket left behind is stale. A daemon that holds it and does
            // not answer is starting or ending, and is waited for.
            if !started && let Some(lock) = Lock::try_take(&environment.layout.daemon_lock())? {
                let _ = std::fs::remove_file(environment.socket());
                drop(lock);
                let environment = environment.clone();
                match tokio::task::spawn_blocking(move || start::start(&environment))
                    .await
                    .expect("starting the daemon does not panic")?
                {
                    Started::Ready | Started::Held => started = true,
                }
                continue;
            }
            if Instant::now() >= until {
                return Err(ClientError::NoDaemon(environment.layout.daemon_log()));
            }
            tokio::time::sleep(RETRY).await;
        }
    }

    /// This client, when its daemon is of this tay. A daemon of an older
    /// tay is asked to exit and is waited for; one of a newer tay is an
    /// error.
    async fn of_this_tay(self) -> Result<Option<Client>, ClientError> {
        if self.daemon.tay == THIS_TAY {
            return Ok(Some(self));
        }
        let older = matches!(
            (parse_version(&self.daemon.tay), parse_version(THIS_TAY)),
            (Some(theirs), Some(ours)) if theirs < ours
        );
        if !older {
            return Err(ClientError::Newer {
                theirs: self.daemon.tay,
            });
        }
        self.exit().await?;
        Ok(None)
    }

    /// Brings up what `request` asks for, giving each event to `event` as
    /// the daemon says it.
    pub async fn up(
        &self,
        request: &UpRequest,
        event: &mut dyn FnMut(Event),
    ) -> Result<(), ClientError> {
        let body = serde_json::to_vec(request).expect("a request serializes");
        self.change(protocol::UP, body, event).await
    }

    /// Takes the environment down, giving each event to `event`, and waits
    /// for the daemon to exit.
    pub async fn down(&self, event: &mut dyn FnMut(Event)) -> Result<(), ClientError> {
        self.change(protocol::DOWN, Vec::new(), event).await?;
        self.gone().await
    }

    pub async fn status(&self) -> Result<Status, ClientError> {
        let (status, body) = http::ask(
            &self.socket,
            Method::GET,
            protocol::STATUS,
            Vec::new(),
            http::ANSWER,
        )
        .await?;
        parse(status, &body)
    }

    /// Asks the daemon to exit, leaving the stack as it is, and waits until
    /// it has.
    async fn exit(&self) -> Result<(), ClientError> {
        let (status, body) = http::ask(
            &self.socket,
            Method::POST,
            protocol::EXIT,
            Vec::new(),
            http::WHO,
        )
        .await?;
        // One that is exiting already is doing what was asked of it.
        if status != StatusCode::SERVICE_UNAVAILABLE {
            let _: serde_json::Value = parse(status, &body)?;
        }
        self.gone().await
    }

    /// Asks for a change and follows it until the daemon has said its last.
    async fn change(
        &self,
        path: &str,
        body: Vec<u8>,
        event: &mut dyn FnMut(Event),
    ) -> Result<(), ClientError> {
        let response = http::send(&self.socket, Method::POST, path, body).await?;
        let status = response.status();
        if status != StatusCode::OK {
            let body = http_body_util::BodyExt::collect(response.into_body())
                .await
                .map_err(|error| NoAnswer::Broken(error.to_string()))?
                .to_bytes();
            return Err(match status {
                StatusCode::CONFLICT => ClientError::Refused(parse(StatusCode::OK, &body)?),
                _ => parse::<serde_json::Value>(status, &body)
                    .err()
                    .unwrap_or_else(|| ClientError::Garbled(status.to_string())),
            });
        }
        let mut ended = false;
        let mut garbled = None;
        http::lines(
            response,
            &mut |line| match serde_json::from_slice::<Event>(line) {
                Ok(said) => {
                    ended = said.ends_change();
                    event(said);
                }
                Err(error) => {
                    garbled.get_or_insert_with(|| error.to_string());
                }
            },
        )
        .await?;
        if let Some(garbled) = garbled {
            return Err(ClientError::Garbled(garbled));
        }
        match ended {
            true => Ok(()),
            false => Err(ClientError::WentAway),
        }
    }

    /// Waits for the daemon's process to end. Its number came from the
    /// daemon itself, so it is not a stale one from a file.
    async fn gone(&self) -> Result<(), ClientError> {
        let until = Instant::now() + START;
        let pid = i32::try_from(self.daemon.pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw);
        // Only "no such process" says it has gone; a process of another
        // account would be refused the signal, and is there.
        let there = |pid| rustix::process::test_kill_process(pid) != Err(Errno::SRCH);
        while pid.is_some_and(there) {
            if Instant::now() >= until {
                return Err(ClientError::StillRunning {
                    pid: self.daemon.pid,
                });
            }
            tokio::time::sleep(RETRY).await;
        }
        Ok(())
    }
}

fn parse<T: DeserializeOwned>(status: StatusCode, body: &[u8]) -> Result<T, ClientError> {
    if status == StatusCode::SERVICE_UNAVAILABLE {
        return Err(ClientError::Exiting);
    }
    if status != StatusCode::OK {
        let problem = serde_json::from_slice::<Problem>(body)
            .map_or_else(|_| status.to_string(), |problem| problem.problem);
        return Err(ClientError::Garbled(problem));
    }
    serde_json::from_slice(body).map_err(|error| ClientError::Garbled(error.to_string()))
}
