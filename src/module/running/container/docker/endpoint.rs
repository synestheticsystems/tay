//! Where Docker listens: the local socket Docker's own configuration points
//! at, found as the `docker` command finds it. `DOCKER_HOST` first; then
//! the context `DOCKER_CONTEXT` names, or the one Docker's configuration
//! file names as current, read from `DOCKER_CONFIG` or `~/.docker`; and
//! with neither, Docker's own default socket. tay reads Docker's
//! configuration and never writes it.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Docker's socket when nothing names another.
const DEFAULT: &str = "/var/run/docker.sock";

/// The context that is Docker's default socket.
const DEFAULT_CONTEXT: &str = "default";

#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    #[error(
        "tay talks to Docker over a local socket, and Docker's configuration names `{0}`; set DOCKER_HOST to a `unix://` socket"
    )]
    NotLocal(String),
    #[error("Docker's configuration `{}` cannot be read: {why}", path.display())]
    Unreadable { path: PathBuf, why: String },
    #[error("Docker's configuration names the context `{0}`, which it does not have")]
    NoContext(String),
}

/// Docker's configuration file, as far as tay reads it.
#[derive(Deserialize)]
struct Config {
    #[serde(rename = "currentContext")]
    current_context: Option<String>,
}

/// A context's description, as far as tay reads it.
#[derive(Deserialize)]
struct Meta {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Endpoints")]
    endpoints: Endpoints,
}

#[derive(Deserialize)]
struct Endpoints {
    docker: Option<Host>,
}

#[derive(Deserialize)]
struct Host {
    #[serde(rename = "Host")]
    host: Option<String>,
}

/// The path of the socket Docker listens on, as this process's environment
/// and Docker's configuration say.
pub fn socket() -> Result<PathBuf, EndpointError> {
    if let Some(host) = std::env::var("DOCKER_HOST")
        .ok()
        .filter(|host| !host.is_empty())
    {
        return local(&host);
    }
    let Some(config) = config_folder() else {
        return Ok(PathBuf::from(DEFAULT));
    };
    let context = match std::env::var("DOCKER_CONTEXT")
        .ok()
        .filter(|name| !name.is_empty())
    {
        Some(context) => Some(context),
        None => current_context(&config.join("config.json"))?,
    };
    match context.as_deref() {
        None | Some(DEFAULT_CONTEXT) => Ok(PathBuf::from(DEFAULT)),
        Some(context) => local(&context_host(&config, context)?),
    }
}

/// The folder of Docker's configuration: `DOCKER_CONFIG`, or `~/.docker`.
fn config_folder() -> Option<PathBuf> {
    match std::env::var_os("DOCKER_CONFIG").filter(|folder| !folder.is_empty()) {
        Some(folder) => Some(PathBuf::from(folder)),
        None => std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".docker")),
    }
}

/// The context the configuration file names as current; `None` when there
/// is no file, or it names none.
fn current_context(path: &Path) -> Result<Option<String>, EndpointError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(path, &error)),
    };
    let config: Config = serde_json::from_str(&text).map_err(|error| unreadable(path, &error))?;
    Ok(config.current_context.filter(|name| !name.is_empty()))
}

/// The Docker host of the context `name`. Docker keeps each context's
/// description in a folder named by a digest of its name, so each is read
/// until the one of that name is found.
fn context_host(config: &Path, name: &str) -> Result<String, EndpointError> {
    let folder = config.join("contexts").join("meta");
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(EndpointError::NoContext(name.to_string()));
        }
        Err(error) => return Err(unreadable(&folder, &error)),
    };
    for entry in entries {
        let path = entry
            .map_err(|error| unreadable(&folder, &error))?
            .path()
            .join("meta.json");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(unreadable(&path, &error)),
        };
        let meta: Meta = serde_json::from_str(&text).map_err(|error| unreadable(&path, &error))?;
        if meta.name != name {
            continue;
        }
        return meta
            .endpoints
            .docker
            .and_then(|docker| docker.host)
            .ok_or_else(|| EndpointError::NoContext(name.to_string()));
    }
    Err(EndpointError::NoContext(name.to_string()))
}

/// The socket a host names; only a `unix://` one is local.
fn local(host: &str) -> Result<PathBuf, EndpointError> {
    match host.strip_prefix("unix://") {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => Err(EndpointError::NotLocal(host.to_string())),
    }
}

fn unreadable(path: &Path, why: &dyn std::fmt::Display) -> EndpointError {
    EndpointError::Unreadable {
        path: path.to_path_buf(),
        why: why.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_unix_socket_is_local() {
        assert_eq!(
            local("unix:///run/docker.sock").unwrap(),
            PathBuf::from("/run/docker.sock")
        );
        for host in ["tcp://localhost:2375", "ssh://box", "unix://"] {
            assert!(
                matches!(local(host), Err(EndpointError::NotLocal(_))),
                "{host}"
            );
        }
    }

    #[test]
    fn a_context_is_found_by_its_name_whatever_its_folder_is_called() {
        let config = tempfile::tempdir().unwrap();
        for (folder, name, host) in [
            ("aaaa", "other", "unix:///other.sock"),
            ("bbbb", "desktop", "unix:///desktop.sock"),
        ] {
            let meta = config.path().join("contexts/meta").join(folder);
            std::fs::create_dir_all(&meta).unwrap();
            let json = format!(
                r#"{{"Name":"{name}","Metadata":{{}},"Endpoints":{{"docker":{{"Host":"{host}","SkipTLSVerify":false}}}}}}"#
            );
            std::fs::write(meta.join("meta.json"), json).unwrap();
        }
        assert_eq!(
            context_host(config.path(), "desktop").unwrap(),
            "unix:///desktop.sock"
        );
        assert!(matches!(
            context_host(config.path(), "missing"),
            Err(EndpointError::NoContext(_))
        ));
        std::fs::write(
            config.path().join("config.json"),
            r#"{"auths":{},"currentContext":"desktop"}"#,
        )
        .unwrap();
        assert_eq!(
            current_context(&config.path().join("config.json")).unwrap(),
            Some("desktop".to_string())
        );
    }
}
