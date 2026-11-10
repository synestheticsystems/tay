//! HTTP over the daemon's Unix socket: one request to a connection.

use std::path::Path;
use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::client::conn::http1;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

/// How long a daemon may take to say who it is: it says so at once, so one
/// that does not is not answering.
pub const WHO: Duration = Duration::from_secs(2);

/// How long any other answer that is not a change's may take, such as one
/// that checks every unit.
pub const ANSWER: Duration = Duration::from_secs(30);

/// Why a request had no answer.
#[derive(Debug, thiserror::Error)]
pub enum NoAnswer {
    #[error("nothing listens on `{}`", .0.display())]
    NotListening(std::path::PathBuf),
    #[error("the daemon did not answer: {0}")]
    Broken(String),
    #[error("the daemon did not answer within {} seconds", .0.as_secs())]
    TooLate(Duration),
}

/// Sends a request, and gives the answer's status as soon as it comes; its
/// body comes as the daemon sends it.
pub async fn send(
    socket: &Path,
    method: Method,
    path: &str,
    body: Vec<u8>,
) -> Result<Response<Incoming>, NoAnswer> {
    let stream = match UnixStream::connect(socket).await {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(NoAnswer::NotListening(socket.to_path_buf()));
        }
        Err(error) => return Err(NoAnswer::Broken(error.to_string())),
    };
    let broken = |error: hyper::Error| NoAnswer::Broken(error.to_string());
    let (mut sender, connection) = http1::handshake(TokioIo::new(stream))
        .await
        .map_err(broken)?;
    // The connection is driven apart from the request; when it fails, the
    // request and its answer's body fail with it.
    tokio::spawn(connection);
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "tay")
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body)))
        .expect("a request of a method, a path and a body is sound");
    sender.send_request(request).await.map_err(broken)
}

/// Sends a request and reads the whole answer, within `within`.
pub async fn ask(
    socket: &Path,
    method: Method,
    path: &str,
    body: Vec<u8>,
    within: Duration,
) -> Result<(StatusCode, Bytes), NoAnswer> {
    let answer = async {
        let response = send(socket, method, path, body).await?;
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|error| NoAnswer::Broken(error.to_string()))?;
        Ok((status, body.to_bytes()))
    };
    tokio::time::timeout(within, answer)
        .await
        .map_err(|_| NoAnswer::TooLate(within))?
}

/// Reads an answer's body a line at a time, as the daemon sends it, giving
/// each line to `line`. Ends when the body does.
pub async fn lines(
    response: Response<Incoming>,
    line: &mut dyn FnMut(&[u8]),
) -> Result<(), NoAnswer> {
    let mut body = response.into_body();
    let mut pending = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| NoAnswer::Broken(error.to_string()))?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        pending.extend_from_slice(&data);
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let rest = pending.split_off(end + 1);
            line(&pending[..end]);
            pending = rest;
        }
    }
    Ok(())
}
