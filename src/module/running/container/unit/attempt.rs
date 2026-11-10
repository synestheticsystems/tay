//! A container unit's attempt. Each attempt runs in a container created for
//! it: the network is made if missing, the image pulled if it is not here or
//! built, the unit's volumes made, the container left by an earlier attempt
//! removed once what remains of its output is copied, and the new one
//! created from the unit's settings and started. Then its output is copied
//! into the unit's log for as long as it runs, and its readiness judged,
//! until it ends. A daemon that did not start it finds it by its name and
//! the environment's label.

use std::path::Path;
use std::time::Duration;

use bollard::models::ContainerStateStatusEnum;

use super::create::{self, ImageHere};
use super::readiness::judge;
use super::{ContainerUnit, volumes};
use crate::clock;
use crate::disk::unit_log::{Adding, Writer};
use crate::module::operations::{self, Failure, Found, Outcome, Step, Steps, UnderWay, UnitPlace};
use crate::module::running::container::docker::ours::ATTEMPT;
use crate::module::running::container::docker::{
    Build, Docker, DockerError, OurContainer, Owner, copy_output, runs,
};
use crate::module::running::container::ended::Ended;
use crate::module::running::container::left::{make_network, remove_copied};
use crate::module::running::container::{
    Image, container_name, copied, make_mount_folders, unit_log,
};

/// The step of a container's attempt, in a daemon's words: the container
/// runs.
const RUNS: &str = "container";

/// How long a container's output that ended while it goes on is waited
/// for before it is asked for again.
const AGAIN: Duration = Duration::from_millis(500);

/// How many times watching a container that fails is begun again, and how
/// long after each failure.
const WATCH_TRIES: u32 = 5;
const WATCH_AGAIN: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
enum AttemptError {
    #[error("its container carries no attempt that tay reads: it has `{0}`")]
    NoAttempt(String),
    #[error("its image `{0}` is not here after it was made")]
    NoImage(String),
}

impl ContainerUnit {
    pub(super) async fn attempt(
        &self,
        place: &UnitPlace,
        attempt: u32,
        steps: &Steps,
    ) -> Result<Outcome, Failure> {
        let docker = Docker::connect()?;
        let id = place.environment.id.as_str();
        let network = make_network(&docker, &place.environment).await?;
        let mut log = unit_log(place)?;
        let image = self.image(&docker, place, steps, &mut log).await?;
        volumes::make(&docker, &self.settings, place, &image.name).await?;
        make_mount_folders(&self.settings.mounts, place)?;
        let name = container_name(place);
        if let Some((left, _)) = docker.container(id, &name).await? {
            remove_copied(&docker, place, left, &mut log).await?;
        }
        let mut creation = create::creation(
            &self.settings,
            &self.opened,
            place,
            &image,
            network.gateway.as_deref(),
        )?;
        let labels = creation.body.labels.get_or_insert_default();
        labels.insert(ATTEMPT.to_string(), attempt.to_string());
        let owner = Owner {
            environment: id,
            unit: Some(&place.unit),
        };
        let container = docker
            .create(
                &owner,
                &name,
                creation.body,
                creation.platform.as_deref(),
                &creation.secrets,
            )
            .await?;
        note(
            &mut log,
            &format!(
                "attempt {attempt}, container: starting `{name}` from `{}`",
                image.name
            ),
        )?;
        // The engine keeps what it was started from, unless it has stopped
        // listening, which it does only once it has dropped the attempt.
        let started = create::fingerprint(&self.settings, &self.opened, place, &image.id);
        let _ = steps.send(Step::Started(started));
        match docker.start(&container).await {
            Ok(()) => {}
            // What Docker cannot start, starting it again does not mend.
            Err(DockerError::Refused { why, .. }) => {
                note(
                    &mut log,
                    &format!("attempt {attempt}, container: could not be started: {why}"),
                )?;
                return Ok(Outcome::Failed(format!(
                    "its container could not be started: {why}"
                )));
            }
            Err(error) => return Err(error.into()),
        }
        watched(&docker, place, &container, attempt, steps, &mut log).await
    }

    /// The image as the container names it: the one its settings give, or
    /// the tag tay builds it as.
    pub(super) fn image_name(&self, place: &UnitPlace) -> Result<String, Failure> {
        Ok(match &self.settings.image {
            Image::Named(name) => operations::text(name, "image")?.to_string(),
            Image::Built { .. } => format!("tay-{}-{}", place.environment.id, place.unit),
        })
    }

    /// The unit's image, pulled if it is not here, or built: either says
    /// how far it has come in the unit's detail, and a build's own words
    /// go to the unit's log.
    async fn image(
        &self,
        docker: &Docker,
        place: &UnitPlace,
        steps: &Steps,
        log: &mut Adding,
    ) -> Result<ImageHere, Failure> {
        let name = self.image_name(place)?;
        let platform = self
            .settings
            .platform
            .as_ref()
            .map(|platform| operations::text(platform, "platform"))
            .transpose()?;
        // The engine has stopped listening only once it has dropped the
        // attempt, which then goes no further.
        let mut progress = |doing: String| {
            let _ = steps.send(Step::Before(doing));
        };
        match &self.settings.image {
            Image::Named(_) => {
                if docker.image_id(&name).await?.is_none() {
                    docker.pull(&name, platform, &mut progress).await?;
                }
            }
            Image::Built {
                context,
                dockerfile,
                args,
            } => {
                let args = args
                    .iter()
                    .map(|(arg, value)| Ok((arg.clone(), operations::scalar(value, "build")?)))
                    .collect::<Result<Vec<_>, Failure>>()?;
                let dockerfile = dockerfile
                    .as_ref()
                    .map(|dockerfile| operations::text(dockerfile, "build"))
                    .transpose()?;
                let build = Build {
                    tag: &name,
                    context: Path::new(operations::text(context, "build")?),
                    dockerfile,
                    args: &args,
                    platform,
                };
                // A line of the build that cannot be written to the log is
                // lost; the build goes on, and says how it ended.
                let mut line = |said: &str| {
                    let _ = log.write(Writer::Tay, &format!("build: {said}"));
                };
                docker.build(&build, &mut progress, &mut line).await?;
            }
        }
        let id = docker
            .image_id(&name)
            .await?
            .ok_or_else(|| AttemptError::NoImage(name.clone()))?;
        Ok(ImageHere { name, id })
    }
}

/// Watches `container`, which runs attempt `attempt`, until it ends: its
/// output is copied into `log`, and its own health judged, for as long as it
/// runs. Watching that fails, as it does while Docker restarts, is begun
/// again from where it stopped, a few times; a container that cannot be
/// watched for that long is stopped, so that none is left with nothing
/// looking after it, and the attempt has ended.
pub(super) async fn watched(
    docker: &Docker,
    place: &UnitPlace,
    container: &OurContainer,
    attempt: u32,
    steps: &Steps,
    log: &mut Adding,
) -> Result<Outcome, Failure> {
    let Some(inspected) = docker.inspect(container).await? else {
        return Ok(gone());
    };
    let since = inspected
        .state
        .as_ref()
        .and_then(|state| state.started_at.as_deref())
        .and_then(clock::nanos_of)
        .map_or_else(clock::now_millis, |nanos| nanos / 1_000_000);
    // The engine has stopped listening only once it has dropped the
    // attempt, which then goes no further.
    let _ = steps.send(Step::Running { since });
    let mut failed = 0;
    let failure = loop {
        match watch(docker, place, container, attempt, steps, log).await {
            Ok(outcome) => return Ok(outcome),
            Err(failure) if failed == WATCH_TRIES => break failure,
            Err(_) => {
                failed += 1;
                tokio::time::sleep(WATCH_AGAIN).await;
            }
        }
    };
    // Its `restart` decides what follows; one whose container cannot be
    // stopped cannot be started again.
    Ok(match crate::module::running::container::stop(place).await {
        Ok(()) => Outcome::Ended {
            failed: true,
            detail: format!("could no longer be watched, and was stopped: {failure}"),
            exit_status: None,
        },
        Err(why) => Outcome::Failed(format!(
            "attempt {attempt} could no longer be watched ({failure}), and its container could not be stopped: {why}"
        )),
    })
}

/// Watches `container` once, until it ends or watching it fails.
async fn watch(
    docker: &Docker,
    place: &UnitPlace,
    container: &OurContainer,
    attempt: u32,
    steps: &Steps,
    log: &mut Adding,
) -> Result<Outcome, Failure> {
    let Some(mut inspected) = docker.inspect(container).await? else {
        return Ok(gone());
    };
    let copied = copied(place);
    while runs(&inspected) {
        // Docker ends the output it gives once the container stops.
        tokio::select! {
            copying = copy_output(docker, container, log, &copied, true) => copying?,
            never = judge(docker, container, steps) => match never {},
        }
        let Some(now) = docker.inspect(container).await? else {
            return Ok(gone());
        };
        inspected = now;
        // Output that ends while the container goes on is asked for again,
        // after a pause, so that a Docker that ends it at once is not asked
        // without end.
        if runs(&inspected) {
            tokio::time::sleep(AGAIN).await;
        }
    }
    let ended = Ended::of(&inspected);
    note(
        log,
        &format!("attempt {attempt}, container: {}", ended.how()),
    )?;
    Ok(Outcome::Ended {
        failed: !ended.succeeded(),
        detail: ended.words(),
        exit_status: Some(ended.exit_status()),
    })
}

/// The unit's container as a daemon that did not start it finds it, by its
/// name and the environment's label. One that has ended has what remains
/// of its output copied, so that what it wrote while no daemon ran is in
/// its log at once.
pub(super) async fn found(place: &UnitPlace) -> Result<Found, Failure> {
    let docker = Docker::connect()?;
    let id = place.environment.id.as_str();
    let Some((container, inspected)) = docker.container(id, &container_name(place)).await? else {
        return Ok(Found::Never);
    };
    let attempt = container.label(ATTEMPT).unwrap_or("none");
    let attempt = attempt
        .parse::<u32>()
        .map_err(|_| AttemptError::NoAttempt(attempt.to_string()))?;
    if runs(&inspected) {
        return Ok(Found::UnderWay(UnderWay {
            attempt,
            step: RUNS.to_string(),
        }));
    }
    let mut log = unit_log(place)?;
    copy_output(&docker, &container, &mut log, &copied(place), false).await?;
    let created = matches!(
        inspected.state.as_ref().and_then(|state| state.status),
        Some(ContainerStateStatusEnum::CREATED)
    );
    let outcome = match created {
        true => Outcome::Ended {
            failed: true,
            detail: "was created, and its daemon ended before it was started".to_string(),
            exit_status: None,
        },
        false => {
            let ended = Ended::of(&inspected);
            Outcome::Ended {
                failed: !ended.succeeded(),
                detail: format!("{}, while no daemon ran", ended.words()),
                exit_status: Some(ended.exit_status()),
            }
        }
    };
    Ok(Found::Ended { attempt, outcome })
}

/// The outcome of an attempt whose container went while it was watched.
pub(super) fn gone() -> Outcome {
    Outcome::Ended {
        failed: true,
        detail: "its container was removed by something other than tay".to_string(),
        exit_status: None,
    }
}

fn note(log: &mut Adding, text: &str) -> Result<(), Failure> {
    Ok(log.write(Writer::Tay, text)?)
}
