//! What of an environment's units is left in Docker, found by the
//! environment's label whatever their settings, and stopped from there;
//! and what of the environment itself is: its network, removed once its
//! units are. Nothing is in a Docker that does not run.

use super::docker::{Docker, DockerError, OurContainer, copy_output, ours};
use super::{container_name, copied};
use crate::disk::Environment;
use crate::disk::unit_log::{Adding, UnitLog};
use crate::module::description::Runs;
use crate::module::operations::{Failure, UnitPlace};

/// Whether anything of `place`'s unit is left in Docker: a container of
/// it, running or ended. Nothing is in a Docker that does not run.
pub async fn anything_left(place: &UnitPlace) -> Result<bool, Failure> {
    let Some(docker) = Docker::if_running().await? else {
        return Ok(false);
    };
    let id = place.environment.id.as_str();
    Ok(!docker.containers(id, Some(&place.unit)).await?.is_empty())
}

/// The units of `environment` of which a container is left in Docker, and
/// where each runs.
pub async fn found_left(environment: &Environment) -> Result<Vec<(String, Runs)>, Failure> {
    let Some(docker) = Docker::if_running().await? else {
        return Ok(Vec::new());
    };
    let mut found: Vec<(String, Runs)> = Vec::new();
    for container in docker.containers(environment.id.as_str(), None).await? {
        if found.iter().any(|(unit, _)| unit == container.unit()) {
            continue;
        }
        let runs = match container.label(ours::STEP) {
            Some(_) => Runs::OneOffContainer,
            None => Runs::OwnContainer,
        };
        found.push((container.unit().to_string(), runs));
    }
    found.sort_by(|(one, _), (other, _)| one.cmp(other));
    Ok(found)
}

/// Stops and removes every container of `place`'s unit, whatever its
/// settings, its own as [`remove_copied`] does.
pub async fn stop(place: &UnitPlace) -> Result<(), Failure> {
    let Some(docker) = Docker::if_running().await? else {
        return Ok(());
    };
    let id = place.environment.id.as_str();
    let name = container_name(place);
    for container in docker.containers(id, Some(&place.unit)).await? {
        match container.name() == name {
            true => {
                let mut log = UnitLog::at(place.environment.layout.unit_log(&place.unit)).open()?;
                remove_copied(&docker, place, container, &mut log).await?;
            }
            false => docker.remove(container).await?,
        }
    }
    Ok(())
}

/// Removes `container`, the unit's own at `place`, once it is stopped and
/// what remains of its output, what it said as it stopped included, is
/// copied into `log`. One whose output cannot be copied is left stopped,
/// and said so, so that nothing it wrote is lost.
pub async fn remove_copied(
    docker: &Docker,
    place: &UnitPlace,
    container: OurContainer,
    log: &mut Adding,
) -> Result<(), Failure> {
    docker.stop(&container).await?;
    if let Err(why) = copy_output(docker, &container, log, &copied(place), false).await {
        return Err(DockerError::Other(format!(
            "its container was stopped, and is kept, since what remained of its output could not be copied: {why}"
        ))
        .into());
    }
    Ok(docker.remove(container).await?)
}

/// Removes the environment's network, once its containers are gone.
pub async fn remove_shared(environment: &Environment) -> Result<(), Failure> {
    let Some(docker) = Docker::if_running().await? else {
        return Ok(());
    };
    let id = environment.id.as_str();
    if let Some(network) = docker.network(id, &environment.network()).await? {
        docker.remove_network(network).await?;
    }
    Ok(())
}
