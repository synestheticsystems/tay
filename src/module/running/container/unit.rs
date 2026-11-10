//! A container unit: its settings filled, and the ports the plan opens for
//! it, run through Docker. Each attempt runs in a container created for it,
//! as [`attempt`] says; its readiness is judged as [`readiness`] says; and
//! once it ends Docker says how, as [`Ended`](super::ended::Ended) words it. Its `restart`
//! decides what follows, as for a process.

mod attempt;
mod create;
mod readiness;
mod volumes;

use std::sync::Arc;
use std::time::Duration;

use super::docker::Docker;
use super::{ContainerSettings, container_name, unit_log};
use crate::module::description::OpenedPort;
use crate::module::operations::{
    Driving, Failure, Finding, Fingerprinting, Outcome, ReadinessCommand, Running, Steps, UnderWay,
    UnitPlace,
};
use crate::value::Filled;
use attempt::{found, gone, watched};

pub use readiness::check_inside;

/// A container unit, its settings filled, and the ports the plan opens for
/// it.
pub struct ContainerUnit {
    pub settings: ContainerSettings<Filled>,
    pub opened: Vec<OpenedPort<Filled>>,
}

impl Running for ContainerUnit {
    fn start(self: Arc<Self>, place: UnitPlace, attempt: u32, steps: Steps) -> Driving {
        Box::pin(async move {
            self.attempt(&place, attempt, &steps)
                .await
                .unwrap_or_else(|failure| Outcome::Failed(failure.to_string()))
        })
    }

    fn found(self: Arc<Self>, place: UnitPlace) -> Finding {
        Box::pin(async move { found(&place).await })
    }

    fn resume(self: Arc<Self>, place: UnitPlace, under_way: UnderWay, steps: Steps) -> Driving {
        Box::pin(async move {
            let resumed = async {
                let docker = Docker::connect()?;
                let found = docker
                    .container(place.environment.id.as_str(), &container_name(&place))
                    .await?;
                let Some((container, _)) = found else {
                    return Ok(gone());
                };
                let mut log = unit_log(&place)?;
                watched(
                    &docker,
                    &place,
                    &container,
                    under_way.attempt,
                    &steps,
                    &mut log,
                )
                .await
            };
            resumed
                .await
                .unwrap_or_else(|failure: Failure| Outcome::Failed(failure.to_string()))
        })
    }

    fn readiness_command(
        self: Arc<Self>,
        place: UnitPlace,
        command: Vec<String>,
        limit: Duration,
    ) -> ReadinessCommand {
        Box::pin(async move { super::check_inside(&place, &command, limit).await })
    }

    fn started_from(self: Arc<Self>, place: UnitPlace) -> Fingerprinting {
        Box::pin(async move {
            let docker = Docker::connect()?;
            let name = self.image_name(&place)?;
            // An image that is not here any more is no image a container
            // was created from.
            let image = docker.image_id(&name).await?.unwrap_or_default();
            Ok(create::fingerprint(
                &self.settings,
                &self.opened,
                &place,
                &image,
            ))
        })
    }
}
