//! One step of a command, `check` or `run`, run in a container: inside the
//! running container of the unit its `in` names, or in a container of its
//! own made from its `image` for that step, on the environment's network,
//! with the step's program in place of the image's entrypoint, and removed
//! once it ends and what it wrote is copied. Either way its secrets reach it
//! through its environment, and the file its `TAY_VALUES` names is fetched
//! from where it ran into the place on the host the command module reads
//! its values from, and how it ended is recorded, since Docker keeps nothing
//! of it once it has ended.

mod record;

use std::path::Path;

use bollard::models::{
    ContainerCreateBody, ContainerStateStatusEnum, HostConfig, NetworkingConfig,
};
use serde::{Deserialize, Serialize};

use super::docker::ours::{ATTEMPT, STEP};
use super::docker::{Docker, Exec, Exited, OurContainer, Owner, copy_output, runs};
use super::ended::Ended;
use super::host::host_entries;
use super::left::{make_network, remove_copied};
use super::{container_name, copied, make_mount_folders, mounted, unit_log};
use crate::clock;
use crate::disk::unit_log::{Adding, Writer};
use crate::disk::{DiskError, write_whole};
use crate::module::operations::{Failure, Step as Told, Steps, UnitPlace};
use crate::module::running::mounts::Mount;
use crate::value::Filled;

/// Where a command's step in a container of its own writes its values,
/// which is fetched from that container before it is removed.
const OWN_VALUES: &str = "/tmp/tay-values.json";

/// A step of a command, as it is run in a container.
pub struct CommandStep {
    pub attempt: u32,
    /// `check` or `run`.
    pub step: &'static str,
    pub argv: Vec<String>,
    /// `NAME=value`, `TAY_VALUES` among them once it is placed.
    pub env: Vec<String>,
    pub secrets: Vec<String>,
    pub user: Option<String>,
    pub workdir: Option<String>,
    pub stdin: Option<Vec<u8>>,
}

/// How a step run in a container ended.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerStepEnd {
    /// Its program exited, as these words say, with this code.
    Exited { code: i64, words: String },
    /// Its program could not be started, as Docker says.
    NotStarted(String),
}

/// A command's last step in a container, as a daemon that did not start
/// it finds it.
pub enum FoundStep {
    Never,
    /// It runs, in a container of the command's own.
    UnderWay {
        attempt: u32,
        step: String,
    },
    /// It ended, and has its values fetched.
    Ended {
        attempt: u32,
        step: String,
        ended: ContainerStepEnd,
    },
}

/// The file inside a unit's running container that a step of a command run
/// in it writes its values to: one of its own each time, so that one left
/// by a step cut short is never read as another's.
fn inside_values(place: &UnitPlace, step: &CommandStep) -> String {
    format!(
        "/tmp/tay-values-{}-{}-{}-{}.json",
        place.unit,
        step.attempt,
        step.step,
        clock::now_millis()
    )
}

/// Runs `step` of the command at `place` inside the running container of
/// `unit`, writing what it prints to the command's log, and fetches its
/// values into `values`; `running` is told when its program has started,
/// when it is the command's own.
pub async fn run_inside(
    place: &UnitPlace,
    unit: &str,
    mut step: CommandStep,
    values: &Path,
    running: Option<&Steps>,
) -> Result<ContainerStepEnd, Failure> {
    let docker = Docker::connect()?;
    let Some((container, _)) = docker
        .container(
            place.environment.id.as_str(),
            &container_name(&place.of(unit)),
        )
        .await?
    else {
        return Ok(ContainerStepEnd::NotStarted(format!(
            "the container of `{unit}`, which it runs in, is not there"
        )));
    };
    let inside = inside_values(place, &step);
    step.env.push(format!("TAY_VALUES={inside}"));
    record::forget(place)?;
    let mut log = unit_log(place)?;
    note(
        &mut log,
        &step,
        &format!(
            "starting `{}` in the container of `{unit}`",
            step.argv.join(" ")
        ),
    )?;
    if let Some(steps) = running {
        // The engine has stopped listening only once it has dropped the
        // attempt, which then goes no further.
        let _ = steps.send(Told::Running {
            since: clock::now_millis(),
        });
    }
    let exec = Exec {
        argv: &step.argv,
        env: &step.env,
        secrets: &step.secrets,
        user: step.user.as_deref(),
        workdir: step.workdir.as_deref(),
        stdin: step.stdin.as_deref(),
    };
    // A line that cannot be written to the log is lost; the step goes on.
    let exited = docker
        .exec(&container, &exec, &mut |line| {
            let _ = log.write(Writer::Unit, line);
        })
        .await;
    let ended = match exited {
        Ok(exited) => ContainerStepEnd::Exited {
            code: exited.code,
            words: format!(
                "exited with {} at {}",
                exited.code,
                clock::utc(clock::now())
            ),
        },
        Err(error) => ContainerStepEnd::NotStarted(error.to_string()),
    };
    note(&mut log, &step, &ended.how())?;
    let fetched = fetch(&docker, &container, &inside, values).await;
    // What it wrote may be a secret, which is not left in another unit's
    // container, whether or not it could be fetched. An image with no `rm`
    // keeps the file, and the log says so.
    let removing = [String::from("rm"), "-f".to_string(), inside.clone()];
    let removed = Exec {
        argv: &removing,
        env: &[],
        secrets: &[],
        user: Some("0"),
        workdir: None,
        stdin: None,
    };
    let left = match docker.exec(&container, &removed, &mut |_| {}).await {
        Ok(Exited { code: 0 }) => None,
        Ok(Exited { code }) => Some(format!("`rm` exited with {code}")),
        Err(error) => Some(error.to_string()),
    };
    if let Some(why) = left {
        note(
            &mut log,
            &step,
            &format!(
                "`{inside}`, which it wrote its values to, could not be removed from the container of `{unit}`: {why}"
            ),
        )?;
    }
    fetched?;
    record::keep(place, step.attempt, step.step, &ended)?;
    Ok(ended)
}

/// Runs `step` of the command at `place` in a container of its own from
/// `image` with `mounts`, copying what it prints into the command's log;
/// `running` is told when its program has started, when it is the
/// command's own. Once it ends, its values are fetched into `values`, and it
/// is removed.
pub async fn run_own(
    place: &UnitPlace,
    image: &str,
    mounts: &[Mount<Filled>],
    mut step: CommandStep,
    values: &Path,
    running: Option<&Steps>,
) -> Result<ContainerStepEnd, Failure> {
    let docker = Docker::connect()?;
    let id = place.environment.id.as_str();
    let network = make_network(&docker, &place.environment).await?;
    if docker.image_id(image).await?.is_none() {
        // The engine has stopped listening only once it has dropped the
        // attempt, which then goes no further.
        let mut progress = |doing: String| {
            if let Some(steps) = running {
                let _ = steps.send(Told::Before(doing));
            }
        };
        docker.pull(image, None, &mut progress).await?;
    }
    let name = container_name(place);
    let mut log = unit_log(place)?;
    if let Some((left, _)) = docker.container(id, &name).await? {
        remove_copied(&docker, place, left, &mut log).await?;
    }
    record::forget(place)?;
    step.env.push(format!("TAY_VALUES={OWN_VALUES}"));
    make_mount_folders(mounts, place)?;
    let mounts = mounts
        .iter()
        .map(|mount| mounted(mount, place))
        .collect::<Result<Vec<_>, Failure>>()?;
    let network_name = place.environment.network();
    let body = ContainerCreateBody {
        image: Some(image.to_string()),
        entrypoint: Some(step.argv.clone()),
        env: Some(step.env.clone()),
        user: step.user.clone(),
        working_dir: step.workdir.clone(),
        open_stdin: Some(step.stdin.is_some()),
        stdin_once: Some(step.stdin.is_some()),
        attach_stdin: Some(step.stdin.is_some()),
        labels: Some(
            [
                (ATTEMPT.to_string(), step.attempt.to_string()),
                (STEP.to_string(), step.step.to_string()),
            ]
            .into(),
        ),
        host_config: Some(HostConfig {
            mounts: Some(mounts),
            network_mode: Some(network_name.clone()),
            extra_hosts: host_entries(network.gateway.as_deref()),
            ..HostConfig::default()
        }),
        networking_config: Some(NetworkingConfig {
            endpoints_config: Some([(network_name, Default::default())].into()),
        }),
        ..ContainerCreateBody::default()
    };
    let owner = Owner {
        environment: id,
        unit: Some(&place.unit),
    };
    let container = docker
        .create(&owner, &name, body, None, &step.secrets)
        .await?;
    note(
        &mut log,
        &step,
        &format!(
            "starting `{}` in `{name}` from `{image}`",
            step.argv.join(" ")
        ),
    )?;
    let fed = match &step.stdin {
        Some(_) => Some(docker.feeding(&container).await?),
        None => None,
    };
    if let Err(error) = docker.start(&container).await {
        docker.remove(container).await?;
        let ended = ContainerStepEnd::NotStarted(error.to_string());
        note(&mut log, &step, &ended.how())?;
        record::keep(place, step.attempt, step.step, &ended)?;
        return Ok(ended);
    }
    if let (Some(fed), Some(stdin)) = (fed, &step.stdin) {
        fed.feed(stdin).await;
    }
    if let Some(steps) = running {
        let _ = steps.send(Told::Running {
            since: clock::now_millis(),
        });
    }
    let ended = waited(
        &docker,
        place,
        container,
        step.attempt,
        step.step,
        values,
        &mut log,
    )
    .await?;
    note(&mut log, &step, &ended.how())?;
    Ok(ended)
}

/// Waits for step `step` of attempt `attempt`, under way in the command's
/// container of its own, to end, as a daemon that did not start it does,
/// then fetches its values into `values` and removes it.
pub async fn wait_own(
    place: &UnitPlace,
    attempt: u32,
    step: &str,
    values: &Path,
    running: Option<&Steps>,
) -> Result<ContainerStepEnd, Failure> {
    let docker = Docker::connect()?;
    let id = place.environment.id.as_str();
    let Some((container, _)) = docker.container(id, &container_name(place)).await? else {
        return Ok(ContainerStepEnd::NotStarted(
            "its container was removed by something other than tay".to_string(),
        ));
    };
    if let Some(steps) = running {
        let _ = steps.send(Told::Running {
            since: clock::now_millis(),
        });
    }
    let mut log = unit_log(place)?;
    waited(&docker, place, container, attempt, step, values, &mut log).await
}

/// The last step of a command run inside another unit's container, as its
/// record says.
pub fn found_inside(place: &UnitPlace) -> Result<FoundStep, Failure> {
    record::found(place)
}

/// The last step of a command run in containers of its own: in its
/// container, found by its name and the environment's label, or as its
/// record says once that container is gone. A container whose step has
/// ended has its values fetched into `values`, and is removed.
pub async fn found_own(place: &UnitPlace, values: &Path) -> Result<FoundStep, Failure> {
    let docker = Docker::connect()?;
    let id = place.environment.id.as_str();
    let Some((container, inspected)) = docker.container(id, &container_name(place)).await? else {
        return record::found(place);
    };
    let attempt = container
        .label(ATTEMPT)
        .and_then(|attempt| attempt.parse::<u32>().ok());
    let step = container.label(STEP).map(str::to_string);
    let (Some(attempt), Some(step)) = (attempt, step) else {
        // A container of the unit that is not one of its steps' is left
        // over from where it ran in another way, and goes.
        docker.remove(container).await?;
        return Ok(FoundStep::Never);
    };
    match inspected.state.as_ref().and_then(|state| state.status) {
        Some(ContainerStateStatusEnum::CREATED) => {
            // Its daemon ended before it started the step, which is simply
            // not done; the record of an earlier step's end no longer
            // speaks for the command.
            docker.remove(container).await?;
            record::forget(place)?;
            return Ok(FoundStep::Never);
        }
        _ if runs(&inspected) => return Ok(FoundStep::UnderWay { attempt, step }),
        _ => {}
    }
    let mut log = unit_log(place)?;
    let ended = waited(&docker, place, container, attempt, &step, values, &mut log).await?;
    Ok(FoundStep::Ended {
        attempt,
        step,
        ended,
    })
}

/// Copies what `container`, which runs step `step` of attempt `attempt`,
/// prints until it ends, then fetches its values into `values`, records how
/// it ended, and removes it; how it ended.
async fn waited(
    docker: &Docker,
    place: &UnitPlace,
    container: OurContainer,
    attempt: u32,
    step: &str,
    values: &Path,
    log: &mut Adding,
) -> Result<ContainerStepEnd, Failure> {
    copy_output(docker, &container, log, &copied(place), true).await?;
    let code = docker.wait(&container).await?;
    let ended = match docker.inspect(&container).await? {
        Some(inspected) => {
            let ended = Ended::of(&inspected);
            ContainerStepEnd::Exited {
                code,
                words: ended.words(),
            }
        }
        None => ContainerStepEnd::Exited {
            code,
            words: format!("exited with {code}"),
        },
    };
    fetch(docker, &container, OWN_VALUES, values).await?;
    // Recorded before the container goes, so that a daemon that ends
    // between the two finds one or the other.
    record::keep(place, attempt, step, &ended)?;
    docker.remove(container).await?;
    Ok(ended)
}

/// Fetches the file at `inside` in `container` to `values` on the host, as
/// the command wrote it; nothing is fetched when it wrote none.
async fn fetch(
    docker: &Docker,
    container: &OurContainer,
    inside: &str,
    values: &Path,
) -> Result<(), Failure> {
    let Some(text) = docker.read_file(container, inside).await? else {
        return Ok(());
    };
    if let Some(folder) = values.parent() {
        std::fs::create_dir_all(folder).map_err(DiskError::io(folder, "made"))?;
    }
    Ok(write_whole(values, &text, 0o600)?)
}

impl ContainerStepEnd {
    /// How it ended, as the command's log notes it.
    fn how(&self) -> String {
        match self {
            ContainerStepEnd::Exited { words, .. } => words.clone(),
            ContainerStepEnd::NotStarted(why) => format!("could not be started: {why}"),
        }
    }
}

/// Notes in the command's log what its step did, as the keeper of a step on
/// the host notes it.
fn note(log: &mut Adding, step: &CommandStep, what: &str) -> Result<(), Failure> {
    Ok(log.write(
        Writer::Tay,
        &format!("attempt {}, {}: {what}", step.attempt, step.step),
    )?)
}
