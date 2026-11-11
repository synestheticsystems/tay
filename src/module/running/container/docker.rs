//! What tay asks of Docker, through its API over the local socket Docker's
//! own configuration points at. Docker is a container's parent: it keeps
//! how the container ended, its output with the time of each line, and
//! what belongs to which environment, by label. So all of that is asked of
//! it, and nothing of it is remembered by tay.
//!
//! The client is private to this module and the modules in it: whatever
//! tay does to a container, a volume or a network goes through the
//! operations here, each of which acts only on what [`ours`] says carries
//! the environment's label.

mod answering;
mod container;
mod endpoint;
mod image;
mod io;
mod network;
pub mod ours;
mod volume;

use std::path::PathBuf;
use std::sync::Mutex;

use bollard::errors::Error as ApiError;

use crate::disk::DiskError;

pub use container::runs;
pub use image::{Build, OurImage};
pub use io::{Exec, Exited, copy_output};
pub use network::OurNetwork;
pub use ours::{OurContainer, Owner};

/// Docker, at its socket.
#[derive(Clone)]
pub struct Docker {
    api: bollard::Docker,
    socket: PathBuf,
}

/// Why something could not be asked of Docker. Its words never hold a
/// secret: what Docker says back is said only for requests that carry
/// none, or with every secret they carry taken out.
#[derive(Debug, thiserror::Error)]
pub enum DockerError {
    #[error(transparent)]
    Endpoint(#[from] endpoint::EndpointError),
    #[error("Docker cannot be reached at `{}`: {why}", socket.display())]
    Unreachable { socket: PathBuf, why: String },
    #[error("Docker could not {doing}: {why}")]
    Refused { doing: String, why: String },
    #[error("{what} exists, and tay did not create it, so tay leaves it alone")]
    Foreign { what: String },
    #[error(transparent)]
    Disk(#[from] DiskError),
    #[error(
        "what remained of its output could not be copied, so its container is kept, stopped: {0}"
    )]
    Uncopied(Box<DockerError>),
    #[error("{0}")]
    Other(String),
}

/// How long a request may take before Docker has answered at all. A pull,
/// a build, a container's output or its wait answer at once and then go on
/// for as long as they take.
const ANSWER: u64 = 120;

impl Docker {
    /// Docker at the socket its configuration points at. Nothing is asked
    /// of it yet. The first that is found is kept for as long as tay runs,
    /// since each request opens a connection of its own.
    pub fn connect() -> Result<Docker, DockerError> {
        static FOUND: Mutex<Option<Docker>> = Mutex::new(None);
        let mut found = FOUND
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(docker) = found.as_ref() {
            return Ok(docker.clone());
        }
        let socket = endpoint::socket()?;
        let path = socket.to_string_lossy();
        let api = bollard::Docker::connect_with_unix(&path, ANSWER, bollard::API_DEFAULT_VERSION)
            .map_err(|error| DockerError::Unreachable {
            socket: socket.clone(),
            why: error.to_string(),
        })?;
        let docker = Docker { api, socket };
        *found = Some(docker.clone());
        Ok(docker)
    }

    /// What became of a request that was `doing` something, as Docker said
    /// it.
    fn failed(&self, doing: impl Into<String>) -> impl FnOnce(ApiError) -> DockerError {
        let socket = self.socket.clone();
        let doing = doing.into();
        move |error| match unreachable(&error) {
            true => DockerError::Unreachable {
                socket,
                why: error.to_string(),
            },
            false => DockerError::Refused {
                doing,
                why: said(&error),
            },
        }
    }
}

/// Whether Docker could not be reached at all, rather than answered.
fn unreachable(error: &ApiError) -> bool {
    match error {
        ApiError::SocketNotFoundError(_) => true,
        ApiError::HyperLegacyError { err } => err.is_connect(),
        ApiError::IOError { err } => matches!(
            err.kind(),
            std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
        ),
        _ => false,
    }
}

/// Whether Docker said that what was asked about does not exist.
fn missing(error: &ApiError) -> bool {
    matches!(
        error,
        ApiError::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

/// Whether Docker said that what was to be made exists already.
fn conflict(error: &ApiError) -> bool {
    matches!(
        error,
        ApiError::DockerResponseServerError {
            status_code: 409,
            ..
        }
    )
}

/// What Docker said, without the code it answered with.
fn said(error: &ApiError) -> String {
    match error {
        ApiError::DockerResponseServerError { message, .. } => message.trim().to_string(),
        other => other.to_string(),
    }
}

/// `error` with each of `secrets` in its words taken out.
pub fn without_secrets(error: DockerError, secrets: &[String]) -> DockerError {
    let scrub = |mut text: String| {
        for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
            text = text.replace(secret.as_str(), "<secret>");
        }
        text
    };
    match error {
        DockerError::Refused { doing, why } => DockerError::Refused {
            doing,
            why: scrub(why),
        },
        DockerError::Unreachable { socket, why } => DockerError::Unreachable {
            socket,
            why: scrub(why),
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_taken_out_of_what_docker_says() {
        let error = DockerError::Refused {
            doing: "create the container `x`".to_string(),
            why: "invalid value hunter2 in hunter2-env".to_string(),
        };
        let said = without_secrets(error, &["hunter2".to_string()]).to_string();
        assert_eq!(
            said,
            "Docker could not create the container `x`: invalid value <secret> in <secret>-env"
        );
    }
}
