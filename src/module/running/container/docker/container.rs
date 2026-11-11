//! An environment's containers, as Docker has them: found by name or by
//! the environment's label, created with it, started, stopped, waited for
//! and removed, each only once [`ours`](super::ours) says it is the
//! environment's.

use std::collections::HashMap;

use bollard::errors::Error as ApiError;
use bollard::models::{ContainerCreateBody, ContainerInspectResponse, ContainerStateStatusEnum};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, InspectContainerOptions, ListContainersOptionsBuilder,
    RemoveContainerOptionsBuilder, StopContainerOptionsBuilder,
};
use futures_util::StreamExt;

use super::ours::{ENVIRONMENT, OurContainer, Owner, UNIT};
use super::{Docker, DockerError, conflict, missing, without_secrets};

/// How long a container is given to stop before Docker kills it, as long
/// as a process is given.
const STOP: i32 = 5;

impl Docker {
    /// The container named `name` and what Docker says of it, when there is
    /// one; one of that name that is not `environment`'s is an error, and is
    /// left alone.
    pub async fn container(
        &self,
        environment: &str,
        name: &str,
    ) -> Result<Option<(OurContainer, ContainerInspectResponse)>, DockerError> {
        let inspected = match self
            .api
            .inspect_container(name, None::<InspectContainerOptions>)
            .await
        {
            Ok(inspected) => inspected,
            Err(error) if missing(&error) => return Ok(None),
            Err(error) => {
                return Err(self.failed(format!("look at the container `{name}`"))(
                    error,
                ));
            }
        };
        let labels = inspected
            .config
            .as_ref()
            .and_then(|config| config.labels.clone());
        match OurContainer::of(
            inspected.id.clone(),
            inspected.name.as_deref(),
            labels,
            environment,
        ) {
            Some(ours) => Ok(Some((ours, inspected))),
            None => Err(DockerError::Foreign {
                what: format!("a container named `{name}`"),
            }),
        }
    }

    /// What Docker says now of `container`; `None` once it has gone.
    pub async fn inspect(
        &self,
        container: &OurContainer,
    ) -> Result<Option<ContainerInspectResponse>, DockerError> {
        match self
            .api
            .inspect_container(&container.id, None::<InspectContainerOptions>)
            .await
        {
            Ok(inspected) => Ok(Some(inspected)),
            Err(error) if missing(&error) => Ok(None),
            Err(error) => Err(self.failed("look at a container")(error)),
        }
    }

    /// Every container, running or not, that is `environment`'s, and
    /// `unit`'s when one is named.
    pub async fn containers(
        &self,
        environment: &str,
        unit: Option<&str>,
    ) -> Result<Vec<OurContainer>, DockerError> {
        let mut labels = vec![format!("{ENVIRONMENT}={environment}")];
        if let Some(unit) = unit {
            labels.push(format!("{UNIT}={unit}"));
        }
        let filters = HashMap::from([("label", labels)]);
        let options = ListContainersOptionsBuilder::default()
            .all(true)
            .filters(&filters)
            .build();
        let listed = self
            .api
            .list_containers(Some(options))
            .await
            .map_err(self.failed("list the environment's containers"))?;
        // Docker filtered them by label; each is looked at again here, so
        // that what decides is this file alone.
        Ok(listed
            .into_iter()
            .filter_map(|summary| {
                let name = summary.names.as_ref().and_then(|names| names.first());
                OurContainer::of(
                    summary.id.clone(),
                    name.map(String::as_str),
                    summary.labels,
                    environment,
                )
            })
            .filter(|container| unit.is_none_or(|unit| container.unit() == unit))
            .collect())
    }

    /// Creates the container `name` from `body`, labelled as `owner`'s, for
    /// `platform` when one is named. `secrets` are the values in its
    /// environment that are secret: what Docker says back is said without
    /// them.
    pub async fn create(
        &self,
        owner: &Owner<'_>,
        name: &str,
        mut body: ContainerCreateBody,
        platform: Option<&str>,
        secrets: &[String],
    ) -> Result<OurContainer, DockerError> {
        let mut labels = body.labels.take().unwrap_or_default();
        // The owner's labels win over any the definition writes, so that a
        // container tay makes is always known to be the environment's.
        labels.extend(owner.labels());
        body.labels = Some(labels.clone());
        let mut options = CreateContainerOptionsBuilder::default().name(name);
        if let Some(platform) = platform {
            options = options.platform(platform);
        }
        let created = self
            .api
            .create_container(Some(options.build()), body)
            .await
            .map_err(|error| {
                let failed = self.failed(format!("create the container `{name}`"))(error);
                without_secrets(failed, secrets)
            })?;
        Ok(OurContainer::created(created.id, name, owner, labels))
    }

    pub async fn start(&self, container: &OurContainer) -> Result<(), DockerError> {
        self.api
            .start_container(&container.id, None)
            .await
            .map_err(self.failed(format!("start the container of `{}`", container.unit())))
    }

    /// Stops `container`, giving it as long to end as a process is given
    /// before Docker kills it. One that has ended already is left so.
    pub async fn stop(&self, container: &OurContainer) -> Result<(), DockerError> {
        let options = StopContainerOptionsBuilder::default().t(STOP).build();
        match self.api.stop_container(&container.id, Some(options)).await {
            Ok(()) => Ok(()),
            // Docker answers 304 for a container that is not running, and
            // 404 for one that has gone.
            Err(error) if missing(&error) => Ok(()),
            Err(error) => Err(self
                .failed(format!("stop the container of `{}`", container.unit()))(
                error
            )),
        }
    }

    /// Removes `container`, stopping it first if it runs, with the volumes
    /// Docker made by itself for it: one with no name for each path its
    /// image declares as a volume and its unit does not name, which belongs
    /// to that container alone and carries no label of tay's. Docker never
    /// removes a named volume with a container, so a unit's own volumes
    /// stay. One that has gone is gone.
    pub async fn remove(&self, container: OurContainer) -> Result<(), DockerError> {
        self.stop(&container).await?;
        let options = RemoveContainerOptionsBuilder::default()
            .force(true)
            .v(true)
            .build();
        match self
            .api
            .remove_container(&container.id, Some(options))
            .await
        {
            Ok(()) => Ok(()),
            Err(error) if missing(&error) => Ok(()),
            // Docker is removing it already.
            Err(error) if conflict(&error) => Ok(()),
            Err(error) => Err(self
                .failed(format!("remove the container of `{}`", container.unit()))(
                error
            )),
        }
    }

    /// Waits for `container` to end, and gives its exit code.
    pub async fn wait(&self, container: &OurContainer) -> Result<i64, DockerError> {
        let mut waiting = self.api.wait_container(&container.id, None);
        match waiting.next().await {
            Some(Ok(ended)) => Ok(ended.status_code),
            // Docker says a code other than 0 as an error of the wait.
            Some(Err(ApiError::DockerContainerWaitError { code, .. })) => Ok(code),
            Some(Err(error)) => Err(self
                .failed(format!("wait for the container of `{}`", container.unit()))(
                error
            )),
            None => Err(DockerError::Other(format!(
                "Docker did not say how the container of `{}` ended",
                container.unit()
            ))),
        }
    }
}

/// Whether Docker says the container runs, or will go on: one that is
/// stopping is watched until it has stopped.
pub fn runs(inspected: &ContainerInspectResponse) -> bool {
    matches!(
        inspected.state.as_ref().and_then(|state| state.status),
        Some(
            ContainerStateStatusEnum::RUNNING
                | ContainerStateStatusEnum::PAUSED
                | ContainerStateStatusEnum::RESTARTING
                | ContainerStateStatusEnum::STOPPING
        )
    )
}
