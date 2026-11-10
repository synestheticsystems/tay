//! The volumes of an environment's units: each made, labelled as its
//! unit's, when it is missing, and used only once it is found to carry the
//! environment's label. `tay down` keeps them: they hold the data.

use std::collections::HashMap;

use bollard::models::VolumeCreateRequest;
use bollard::query_parameters::{ListVolumesOptionsBuilder, RemoveVolumeOptions};

use super::ours::{ENVIRONMENT, Owner, is_ours};
use super::{Docker, DockerError, missing};

impl Docker {
    /// Makes the volume `name`, labelled as `owner`'s, if it is missing;
    /// whether it was made now. One of that name that is not the
    /// environment's is an error, and is left alone.
    pub async fn make_volume(&self, owner: &Owner<'_>, name: &str) -> Result<bool, DockerError> {
        match self.api.inspect_volume(name).await {
            Ok(volume) if is_ours(Some(&volume.labels), owner.environment) => return Ok(false),
            Ok(_) => {
                return Err(DockerError::Foreign {
                    what: format!("a volume named `{name}`"),
                });
            }
            Err(error) if missing(&error) => {}
            Err(error) => return Err(self.failed(format!("look at the volume `{name}`"))(error)),
        }
        let request = VolumeCreateRequest {
            name: Some(name.to_string()),
            labels: Some(owner.labels()),
            ..VolumeCreateRequest::default()
        };
        // Docker answers a create for a name it has with the volume it has,
        // so what it made is the environment's only if it says so.
        let made = self
            .api
            .create_volume(request)
            .await
            .map_err(self.failed(format!("create the volume `{name}`")))?;
        match is_ours(Some(&made.labels), owner.environment) {
            true => Ok(true),
            false => Err(DockerError::Foreign {
                what: format!("a volume named `{name}`"),
            }),
        }
    }

    /// Removes the volume `name`, which [`Docker::make_volume`] has just made as
    /// `owner`'s and which nothing has used yet.
    pub async fn remove_made_volume(
        &self,
        owner: &Owner<'_>,
        name: &str,
    ) -> Result<(), DockerError> {
        if !self.has_volume(owner.environment, name).await? {
            return Ok(());
        }
        self.api
            .remove_volume(name, None::<RemoveVolumeOptions>)
            .await
            .map_err(self.failed(format!("remove the volume `{name}`")))
    }

    /// Whether the volume `name` is there and is `environment`'s; one of
    /// that name that is not is an error.
    pub async fn has_volume(&self, environment: &str, name: &str) -> Result<bool, DockerError> {
        match self.api.inspect_volume(name).await {
            Ok(volume) if is_ours(Some(&volume.labels), environment) => Ok(true),
            Ok(_) => Err(DockerError::Foreign {
                what: format!("a volume named `{name}`"),
            }),
            Err(error) if missing(&error) => Ok(false),
            Err(error) => Err(self.failed(format!("look at the volume `{name}`"))(error)),
        }
    }

    /// The names of the volumes that are `environment`'s.
    pub async fn volumes(&self, environment: &str) -> Result<Vec<String>, DockerError> {
        let filters = HashMap::from([("label", vec![format!("{ENVIRONMENT}={environment}")])]);
        let options = ListVolumesOptionsBuilder::default()
            .filters(&filters)
            .build();
        let listed = self
            .api
            .list_volumes(Some(options))
            .await
            .map_err(self.failed("list the environment's volumes"))?;
        Ok(listed
            .volumes
            .unwrap_or_default()
            .into_iter()
            .filter(|volume| is_ours(Some(&volume.labels), environment))
            .map(|volume| volume.name)
            .collect())
    }
}
