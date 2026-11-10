//! The host, seen from a container: one name on every system, so that a
//! definition never has to know which Docker it runs on, and
//! `${host.listen}`, the address a host process listens on for containers
//! to reach it and other machines not to. Docker Desktop gives the name
//! itself and brings a container's connections to it to the host's own
//! loopback address; on Linux the name means nothing in a container until
//! tay says what it means, the host's address on the environment's network,
//! which is where a host process listens.

#[cfg(not(target_os = "macos"))]
use super::docker::{Docker, DockerError};
use crate::disk::Environment;
use crate::module::operations::Failure;

/// When the engine learns what of the environment its units are filled
/// with: as the environment is brought up, when what that needs may be
/// made, or as a daemon finds it again, when nothing is made.
#[derive(Clone, Copy)]
pub enum Occasion {
    BringingUp,
    FindingAgain,
}

/// The name the host has inside the environment's containers, on every
/// system: Docker Desktop gives it, and on Linux tay does.
pub const HOST_IN_CONTAINERS: &str = "host.docker.internal";

/// `${host.listen}`: the address a host process listens on so that this
/// machine's containers can reach it and other machines cannot. Docker
/// Desktop brings a container's connections to the host's name to the
/// host's own loopback address.
#[cfg(target_os = "macos")]
pub async fn host_listen(_: &Environment, _: Occasion) -> Result<String, Failure> {
    Ok("127.0.0.1".to_string())
}

/// `${host.listen}` elsewhere: the host's address on the environment's
/// network, its gateway, which its containers reach the host by and which
/// no other machine has a way to. The network is made now as the
/// environment is brought up, and is only looked for when a daemon finds
/// the environment again.
#[cfg(not(target_os = "macos"))]
pub async fn host_listen(environment: &Environment, occasion: Occasion) -> Result<String, Failure> {
    let docker = Docker::connect()?;
    let (id, name) = (environment.id.as_str(), environment.network());
    let network = match occasion {
        Occasion::BringingUp => Some(docker.make_network(id, &name).await?),
        Occasion::FindingAgain => docker.network(id, &name).await?,
    };
    match network.and_then(|network| network.gateway) {
        Some(gateway) => Ok(gateway),
        None => Err(DockerError::Other(format!(
            "the environment's network `{name}` is not there, or has no gateway, so the host has no address on it"
        ))
        .into()),
    }
}

/// The entries a container's own list of hosts is given, which say what
/// the host's name means inside it: none, since Docker Desktop gives it
/// itself.
#[cfg(target_os = "macos")]
pub fn host_entries(_: Option<&str>) -> Option<Vec<String>> {
    None
}

/// The entries a container's own list of hosts is given, which say what
/// the host's name means inside it: on Linux Docker gives it no meaning,
/// so it is the host's address on the environment's network, `gateway`.
#[cfg(not(target_os = "macos"))]
pub fn host_entries(gateway: Option<&str>) -> Option<Vec<String>> {
    gateway.map(|gateway| vec![format!("{}:{gateway}", HOST_IN_CONTAINERS)])
}
