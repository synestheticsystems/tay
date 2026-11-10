//! A container's volumes: those its unit owns, made if missing and given to
//! their `owner` when they are made; and those of other units it mounts,
//! which their units make. Mounting another unit's volume orders nothing,
//! so one that is not there yet is not made here, where its owner would
//! not be given it: the unit fails, saying that `needs` orders them.
//!
//! A volume is given to its owner by a container made for that alone from
//! the unit's own image, which runs `chown` on it as root and is removed.

use bollard::models::{ContainerCreateBody, HostConfig, Mount, MountType};

use crate::module::operations::{self, Failure, UnitPlace};
use crate::module::running::container::docker::{Docker, DockerError, Owner};
use crate::module::running::container::{ContainerSettings, container_name, volume_name};
use crate::module::running::mounts::Source;
use crate::value::Filled;

/// Where a volume is mounted in the container that gives it to its owner.
const OWNED_AT: &str = "/tay-volume";

#[derive(Debug, thiserror::Error)]
enum VolumeError {
    #[error(
        "it mounts the volume `{unit}/{volume}`, which is not there yet: `{unit}` makes it when it starts, and `needs` is what orders the two"
    )]
    NotMade { unit: String, volume: String },
    #[error(
        "its volume `{volume}` cannot be given to its owner {owner}: `chown` in its image {how}"
    )]
    NotOwned {
        volume: String,
        owner: i64,
        how: String,
    },
}

/// Makes the volumes `settings` owns that are missing, each given to its
/// owner, and sees that each volume of another unit they mount is there.
/// `image` is the unit's image, which gives a volume to its owner.
pub async fn make(
    docker: &Docker,
    settings: &ContainerSettings<Filled>,
    place: &UnitPlace,
    image: &str,
) -> Result<(), Failure> {
    let id = place.environment.id.as_str();
    let owner = Owner {
        environment: id,
        unit: Some(&place.unit),
    };
    for volume in &settings.volumes {
        let name = volume_name(place, &place.unit, &volume.name.name);
        let made = docker.make_volume(&owner, &name).await?;
        let Some(user) = &volume.owner else {
            continue;
        };
        let user = operations::number(user, "volumes")?;
        if !made {
            continue;
        }
        // A volume that could not be given to its owner is removed, as it
        // was just made and holds nothing, so that the next attempt makes
        // it again.
        if let Err(failure) = give(docker, &owner, place, image, &name, user).await {
            docker.remove_made_volume(&owner, &name).await?;
            return Err(match failure {
                Given::Ended(how) => VolumeError::NotOwned {
                    volume: volume.name.name.clone(),
                    owner: user,
                    how,
                }
                .into(),
                Given::Docker(error) => error.into(),
            });
        }
    }
    for mount in &settings.mounts {
        let Source::Volume(volume) = &mount.source else {
            continue;
        };
        let name = volume_name(place, &volume.unit.name, &volume.volume.name);
        if !docker.has_volume(id, &name).await? {
            return Err(VolumeError::NotMade {
                unit: volume.unit.name.clone(),
                volume: volume.volume.name.clone(),
            }
            .into());
        }
    }
    Ok(())
}

/// Why a volume was not given to its owner.
enum Given {
    /// What gives it ended so.
    Ended(String),
    Docker(DockerError),
}

impl From<DockerError> for Given {
    fn from(error: DockerError) -> Given {
        Given::Docker(error)
    }
}

/// Gives the volume `name` to the numeric user `user`, by a container of
/// `image` that runs `chown` on it as root, and is removed once it ends.
async fn give(
    docker: &Docker,
    owner: &Owner<'_>,
    place: &UnitPlace,
    image: &str,
    name: &str,
    user: i64,
) -> Result<(), Given> {
    let container_name = format!("{}.owner", container_name(place));
    // One left by an attempt that was cut short is the unit's own.
    if let Some((left, _)) = docker.container(owner.environment, &container_name).await? {
        docker.remove(left).await?;
    }
    let body = ContainerCreateBody {
        image: Some(image.to_string()),
        entrypoint: Some(vec!["chown".to_string()]),
        cmd: Some(vec![user.to_string(), OWNED_AT.to_string()]),
        user: Some("0".to_string()),
        host_config: Some(HostConfig {
            network_mode: Some("none".to_string()),
            mounts: Some(vec![Mount {
                typ: Some(MountType::VOLUME),
                source: Some(name.to_string()),
                target: Some(OWNED_AT.to_string()),
                ..Mount::default()
            }]),
            ..HostConfig::default()
        }),
        ..ContainerCreateBody::default()
    };
    let giving = docker
        .create(owner, &container_name, body, None, &[])
        .await?;
    let ended = match docker.start(&giving).await {
        Ok(()) => docker.wait(&giving).await,
        Err(DockerError::Refused { why, .. }) => {
            docker.remove(giving).await?;
            return Err(Given::Ended(format!("could not be started: {why}")));
        }
        Err(error) => Err(error),
    };
    docker.remove(giving).await?;
    match ended? {
        0 => Ok(()),
        code => Err(Given::Ended(format!("exited with {code}"))),
    }
}
