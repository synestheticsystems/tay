//! What of an environment's units is left in Docker, found by the
//! environment's label whatever their settings, and stopped from there;
//! and what of the environment itself is: its network, made and removed
//! here, and anything of it at all, which keeps it on the machine's list.
//!
//! Docker is asked about an environment only once the machine's list notes
//! that the environment has made something there, which it notes before
//! the environment's network, the first thing a container needs, is made.
//! So an environment that never ran a container never needs Docker, and
//! for one that did, a Docker that cannot be reached is said so, rather
//! than taken to hold nothing of it.

use super::docker::{Docker, DockerError, OurContainer, OurNetwork, copy_output, ours};
use super::{container_name, copied, unit_log};
use crate::disk::unit_log::Adding;
use crate::disk::{DiskError, Environment, List, Listed};
use crate::module::description::Runs;
use crate::module::operations::{Failure, UnitPlace};

/// Docker, to be asked about `environment`; `None` when the environment
/// has never made anything there.
fn docker_of(environment: &Environment) -> Result<Option<Docker>, Failure> {
    let made = List::read(&environment.machine)?
        .iter()
        .any(|listed| listed.id == environment.id && listed.made_outside);
    match made {
        true => Ok(Some(Docker::connect()?)),
        false => Ok(None),
    }
}

/// The environment's network, made if it is missing, once the machine's
/// list notes that the environment has made something in Docker.
pub async fn make_network(
    docker: &Docker,
    environment: &Environment,
) -> Result<OurNetwork, Failure> {
    let noted = List::read(&environment.machine)?
        .iter()
        .any(|listed| listed.id == environment.id && listed.made_outside);
    if !noted {
        let machine = environment.machine.clone();
        let id = environment.id.clone();
        // The list is changed under its lock, which may be waited for.
        tokio::task::spawn_blocking(move || -> Result<(), DiskError> {
            let mut list = List::lock(&machine)?;
            match list.note_made_outside(&id)? {
                true => list.save(),
                false => Ok(()),
            }
        })
        .await
        .map_err(|error| {
            DockerError::Other(format!("tay failed while noting it uses Docker: {error}"))
        })?
        .map_err(DockerError::from)?;
    }
    Ok(docker
        .make_network(environment.id.as_str(), &environment.network())
        .await?)
}

/// Whether anything of `place`'s unit is left in Docker: a container of
/// it, running or ended.
pub async fn anything_left(place: &UnitPlace) -> Result<bool, Failure> {
    let Some(docker) = docker_of(&place.environment)? else {
        return Ok(false);
    };
    let id = place.environment.id.as_str();
    Ok(!docker.containers(id, Some(&place.unit)).await?.is_empty())
}

/// The units of `environment` of which a container is left in Docker, and
/// where each runs.
pub async fn found_left(environment: &Environment) -> Result<Vec<(String, Runs)>, Failure> {
    let Some(docker) = docker_of(environment)? else {
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
    let Some(docker) = docker_of(&place.environment)? else {
        return Ok(());
    };
    let id = place.environment.id.as_str();
    let name = container_name(place);
    for container in docker.containers(id, Some(&place.unit)).await? {
        match container.name() == name {
            true => {
                let mut log = unit_log(place)?;
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
        return Err(DockerError::Uncopied(Box::new(why)).into());
    }
    Ok(docker.remove(container).await?)
}

/// Removes the environment's network, once its containers are gone.
pub async fn remove_shared(environment: &Environment) -> Result<(), Failure> {
    let Some(docker) = docker_of(environment)? else {
        return Ok(());
    };
    let id = environment.id.as_str();
    if let Some(network) = docker.network(id, &environment.network()).await? {
        docker.remove_network(network).await?;
    }
    Ok(())
}

/// Whether anything of the environment `listed` is in Docker: a container,
/// a volume or a network that carries its label. Nothing is when it never
/// made anything there.
pub async fn anything_left_of_environment(listed: &Listed) -> Result<bool, Failure> {
    if !listed.made_outside {
        return Ok(false);
    }
    let docker = Docker::connect()?;
    let id = listed.id.as_str();
    Ok(!docker.containers(id, None).await?.is_empty()
        || !docker.volumes(id).await?.is_empty()
        || !docker.networks(id).await?.is_empty())
}
