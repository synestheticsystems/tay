//! What a container keeps: the volumes its unit owns, which hold its data,
//! and its record; and what an environment keeps in Docker, every volume of
//! it. Each volume is removed only once Docker says it carries the
//! environment's label.

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

/// Each volume of `environment`, as a person reads it.
pub async fn kept_by_environment(environment: &Environment) -> Result<Vec<String>, Failure> {
    let Some(docker) = docker_of(environment)? else {
        return Ok(Vec::new());
    };
    let volumes = docker.volumes(environment.id.as_str()).await?;
    Ok(volumes.iter().map(|name| volume_words(name)).collect())
}

/// Removes every volume of `environment`; each that was removed, as a
/// person reads it.
pub async fn remove_kept_by_environment(environment: &Environment) -> Result<Vec<String>, Failure> {
    let Some(docker) = docker_of(environment)? else {
        return Ok(Vec::new());
    };
    let names = docker.volumes(environment.id.as_str()).await?;
    remove_volumes(environment, &names).await?;
    Ok(names.iter().map(|name| volume_words(name)).collect())
}

/// A volume of the environment, as a person reads it.
fn volume_words(name: &str) -> String {
    format!("the volume `{name}`")
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
