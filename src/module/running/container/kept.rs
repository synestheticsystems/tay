//! What a container keeps: the volumes its unit owns, which hold its data,
//! and its record; and what an environment keeps in Docker, every volume of
//! it and every image built for it. Each is removed only once Docker says
//! it carries the environment's label.

use super::docker::OurImage;
use super::left::docker_of;
use super::{ContainerSettings, volume_name};
use crate::disk::Environment;
use crate::module::operations::{Failure, Keeping, PartlyRemoved, Removing, UnitPlace};
use crate::module::running::{RECORD, remove_record};
use crate::module::settings::Keeps;
use crate::value::{Filled, Placeholder, Unfilled, Value};

impl Keeps for ContainerSettings<Value> {
    fn kept(&self, place: &UnitPlace) -> Keeping {
        let volumes = self.volumes.iter().map(|volume| {
            let name = &volume.name.name;
            format!(
                "its volume `{name}`, `{}`",
                volume_name(place, &place.unit, name)
            )
        });
        Keeping {
            what: volumes.chain([RECORD.to_string()]).collect(),
            runs: false,
        }
    }

    fn wipe(
        &self,
        place: UnitPlace,
        _: &mut dyn FnMut(&Placeholder) -> Result<Filled, Unfilled>,
    ) -> Result<Removing, Unfilled> {
        let names: Vec<String> = (self.volumes.iter())
            .map(|volume| volume_name(&place, &place.unit, &volume.name.name))
            .collect();
        Ok(Box::pin(async move {
            for (removed, name) in names.iter().enumerate() {
                remove_volumes(&place.environment, std::slice::from_ref(name))
                    .await
                    .map_err(PartlyRemoved::after(removed))?;
            }
            remove_record(&place).map_err(PartlyRemoved::after(names.len()))
        }))
    }
}

/// Each volume of `environment` and each image built for it, as a person
/// reads it.
pub async fn kept_by_environment(environment: &Environment) -> Result<Vec<String>, Failure> {
    let Some(docker) = docker_of(environment)? else {
        return Ok(Vec::new());
    };
    let id = environment.id.as_str();
    let volumes = docker.volumes(id).await?;
    let images = docker.images(id).await?;
    let volumes = volumes.iter().map(|name| volume_words(name));
    Ok(volumes.chain(images.iter().map(image_words)).collect())
}

/// Removes every volume of `environment` and every image built for it,
/// once nothing of it runs; each that was removed, as a person reads it.
pub async fn remove_kept_by_environment(environment: &Environment) -> Result<Vec<String>, Failure> {
    let Some(docker) = docker_of(environment)? else {
        return Ok(Vec::new());
    };
    let id = environment.id.as_str();
    let names = docker.volumes(id).await?;
    remove_volumes(environment, &names).await?;
    let images = docker.images(id).await?;
    for image in &images {
        docker.remove_image(image).await?;
    }
    let volumes = names.iter().map(|name| volume_words(name));
    Ok(volumes.chain(images.iter().map(image_words)).collect())
}

/// A volume of the environment, as a person reads it.
fn volume_words(name: &str) -> String {
    format!("the volume `{name}`")
}

/// An image built for the environment, as a person reads it.
fn image_words(image: &OurImage) -> String {
    format!("the image `{}`", image.name)
}

/// Removes the volumes `names` of `environment`, each once it is found to
/// be the environment's.
async fn remove_volumes(environment: &Environment, names: &[String]) -> Result<(), Failure> {
    let Some(docker) = docker_of(environment)? else {
        return Ok(());
    };
    for name in names {
        docker.remove_volume(environment.id.as_str(), name).await?;
    }
    Ok(())
}
