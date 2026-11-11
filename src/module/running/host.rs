//! What the units that run on the host share, a process and a command run
//! there: the variables they inherit, the program and environment they are
//! given, running each step under a keeper and waiting for it to end,
//! finding a step again, stopping one, and running a readiness command.
//!
//! A program on the host runs with only the environment its definition
//! gives it and the variables `inherit` names from the environment tay runs
//! in, which is the daemon's, the environment of the command that started
//! it. Secrets reach it only there, never as arguments.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::clock;
use crate::diagnostic::Diagnostics;
use crate::disk::{DiskError, Layout};
use crate::keeper::{Ended, Ending, Look, Program, StepFiles};
use crate::module::operations::{Failure, Outcome, Step, Steps, UnitPlace};
use crate::syntax::yaml::Node;
use crate::syntax::{fields, names};
use crate::value::Filled;

use super::command_line::shown;
use super::variables::Variables;

/// What a program inherits from the environment tay runs in when `inherit`
/// does not say.
const INHERITED: &[&str] = &["PATH", "HOME", "USER", "LANG", "TMPDIR"];

/// How often a readiness command is looked at for its end.
const CHECK_LOOK: Duration = Duration::from_millis(50);

#[derive(Debug, thiserror::Error)]
enum HostError {
    #[error("tay failed while {doing}: {why}")]
    Panicked { doing: &'static str, why: String },
}

/// The files a unit that runs on the host keeps: in its folder of the
/// state folder, its lock, its record and the values it produced; in
/// `logs/<unit>/`, its log.
pub fn files(place: &UnitPlace) -> StepFiles {
    files_of(&place.environment.layout, &place.unit)
}

fn files_of(layout: &Layout, unit: &str) -> StepFiles {
    StepFiles::at(layout.unit_state(unit), layout.unit_log(unit))
}

/// The units of the environment `layout` lays out of which something runs
/// on the host, from each unit's lock and record, whatever the plan has.
pub fn found_running(layout: &Layout) -> Result<Vec<String>, Failure> {
    let folder = layout.units_state();
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(DiskError::io(folder, "read")(error).into()),
    };
    let mut running = Vec::new();
    for entry in entries {
        let entry = entry.map_err(DiskError::io(&folder, "read"))?;
        let Some(unit) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if files_of(layout, &unit).anything_left()? {
            running.push(unit);
        }
    }
    running.sort();
    Ok(running)
}

/// `inherit`, as it is written at `node`: the names of the variables a
/// program on the host is passed from the environment tay runs in, in
/// place of those it inherits when `inherit` does not say.
pub fn inherit(node: &Node, d: &mut Diagnostics) -> Vec<String> {
    fields::name_list(
        node,
        "a variable to inherit",
        names::variable_name_problem,
        d,
    )
    .into_iter()
    .map(|name| name.name)
    .collect()
}

/// The environment a program runs with: each variable `inherit` names that
/// tay's own environment has, then `env`, then `secret_env`, then `more`.
pub fn environment(
    variables: &Variables<Filled>,
    inherit: Option<&[String]>,
    more: &[(&str, String)],
) -> Vec<(String, String)> {
    let inherited: Vec<String> = match inherit {
        Some(names) => names.to_vec(),
        None => INHERITED.iter().map(|name| name.to_string()).collect(),
    };
    let mut env: Vec<(String, String)> = inherited
        .into_iter()
        .filter_map(|name| {
            let value = std::env::var(&name).ok()?;
            Some((name, value))
        })
        .collect();
    let mut set = |name: &str, value: String| {
        env.retain(|(other, _)| other != name);
        env.push((name.to_string(), value));
    };
    for (name, value) in variables.env.iter().chain(&variables.secret_env) {
        set(name, value.reveal());
    }
    for (name, value) in more {
        set(name, value.clone());
    }
    env
}

/// The folder a program runs in: `workdir`, which the planner made a whole
/// path, or the stack's root.
pub fn workdir(workdir: Option<&Filled>, place: &UnitPlace) -> Result<PathBuf, Failure> {
    Ok(match workdir {
        Some(workdir) => PathBuf::from(crate::module::operations::text(workdir, "workdir")?),
        None => place.environment.layout.root().to_path_buf(),
    })
}

/// How a step that was waited for ended.
pub enum StepEnd {
    /// Its keeper said how.
    Ended(Ended),
    /// Its keeper ended without saying how; what was left of its group has
    /// been stopped.
    Lost,
    /// Its record went with the environment's folder; what was left of its
    /// group has been stopped.
    Gone,
}

/// Runs `program` as `step` of attempt `attempt` under a keeper, and waits
/// for it to end. When `running` is given, it is told when the program has
/// started: it is the unit's own.
pub async fn run(
    files: &StepFiles,
    attempt: u32,
    step: &str,
    program: Program,
    running: Option<&Steps>,
) -> Result<StepEnd, Failure> {
    // Starting takes no longer than starting a process, and is done whole
    // between two waits, so that a stop never comes while a keeper is half
    // started.
    files.start(attempt, step, &program)?;
    wait(files, running).await
}

/// Waits for the step under way to end. When `running` is given, it is
/// told when the step's program has started.
pub async fn wait(files: &StepFiles, running: Option<&Steps>) -> Result<StepEnd, Failure> {
    if let Some(steps) = running {
        // A program that has started is told of, whether or not it has
        // ended by the time it is seen. The engine may have stopped
        // listening; the step goes on.
        if let Look::Running(record) | Look::Ended(record, _) | Look::Lost(record) =
            files.child_started().await?
            && record.child.is_some()
        {
            let _ = steps.send(Step::Running {
                since: record.begun,
            });
        }
    }
    let end = match files.ended().await? {
        Look::Ended(_, ended) => return Ok(StepEnd::Ended(ended)),
        Look::Lost(_) => StepEnd::Lost,
        Look::Never | Look::Running(_) => StepEnd::Gone,
    };
    stop_blocking(files.clone()).await?;
    Ok(end)
}

/// Stops what of a unit runs on the host, from its place alone: the step
/// under way, and what is left of a step whose keeper was lost.
pub fn stop(place: &UnitPlace) -> Result<(), Failure> {
    Ok(files(place).stop()?)
}

async fn stop_blocking(files: StepFiles) -> Result<(), Failure> {
    Ok(tokio::task::spawn_blocking(move || files.stop())
        .await
        .map_err(|error| panicked("stopping it", &error))??)
}

/// Whether anything of a unit runs on the host, from its place alone: a
/// step under way, or what is left of the group of one whose keeper was
/// lost.
pub fn running_now(place: &UnitPlace) -> Result<bool, Failure> {
    Ok(files(place).anything_left()?)
}

/// How a program ended, as a person reads it after "attempt 2", with the
/// time.
pub fn ended_words(ended: &Ended) -> String {
    format!("{} at {}", ended.how, clock::utc(ended.at / 1000))
}

/// The outcome of an attempt whose own program ended: a program that could
/// not be started fails the unit, since starting it again would not mend
/// that.
pub fn ending(end: &StepEnd, after: &str) -> Outcome {
    match end {
        StepEnd::Ended(Ended {
            how: how @ Ending::NotStarted { .. },
            ..
        }) => Outcome::Failed(format!("its program {how}")),
        StepEnd::Ended(ended) => Outcome::Ended {
            failed: !ended.how.succeeded(),
            detail: format!("{}{after}", ended_words(ended)),
            exit_status: ended.how.exit_status(),
        },
        StepEnd::Lost => Outcome::Ended {
            failed: true,
            detail: format!(
                "ended with no record of how{after}: its keeper was lost, and what was left of it was stopped"
            ),
            exit_status: None,
        },
        StepEnd::Gone => Outcome::Failed(GONE.to_string()),
    }
}

/// Why an attempt whose record has gone ends, and is not started again.
pub const GONE: &str =
    "its record went with the environment's folder, and what was left of it was stopped";

/// Runs a readiness command on the host, as a child of the daemon in a
/// process group of its own, for as long as `limit`; why it did not pass.
/// A check that is dropped before it ends, as when its unit is stopped,
/// kills what it started.
pub async fn check(program: Program, limit: Duration) -> Result<(), String> {
    let shown = shown(&program.argv);
    let mut command = Command::new(&program.argv[0]);
    command
        .args(&program.argv[1..])
        .env_clear()
        .envs(program.env.iter().map(|(name, value)| (name, value)))
        .current_dir(&program.workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = Checking(
        command
            .spawn()
            .map_err(|error| format!("{shown} cannot be started: {error}"))?,
    );
    let until = Instant::now() + limit;
    loop {
        match child.0.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(match status.code() {
                    Some(code) => format!("{shown} exited with {code}"),
                    None => format!("{shown} was ended by a signal"),
                });
            }
            Ok(None) if Instant::now() >= until => {
                return Err(format!(
                    "{shown} did not end within {} seconds",
                    limit.as_secs()
                ));
            }
            Ok(None) => tokio::time::sleep(CHECK_LOOK).await,
            Err(error) => return Err(format!("{shown} cannot be waited for: {error}")),
        }
    }
}

/// A readiness command the daemon started, killed with all it started and
/// waited for when it is let go, unless it has ended already.
struct Checking(Child);

impl Drop for Checking {
    fn drop(&mut self) {
        if let Ok(Some(_)) = self.0.try_wait() {
            return;
        }
        // The command is the daemon's own child, not yet waited for, so its
        // number and its group are no other's.
        if let Some(group) = rustix::process::Pid::from_raw(self.0.id() as i32) {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
        let _ = self.0.wait();
    }
}

pub fn panicked(doing: &'static str, error: &tokio::task::JoinError) -> Failure {
    HostError::Panicked {
        doing,
        why: error.to_string(),
    }
    .into()
}
