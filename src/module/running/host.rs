//! What the units that run on the host share, a process and a command run
//! there: the program and environment they are given, running each step
//! under a keeper and waiting for it to end, finding a step again, stopping
//! one, and running a readiness command.
//!
//! A program on the host runs with only the environment its definition
//! gives it and the variables `inherit` names from the environment tay runs
//! in, which is the daemon's, the environment of the command that started
//! it. Secrets reach it only there, never as arguments.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::clock;
use crate::keeper::{Ended, Exit, Look, Program, UnitFiles};
use crate::module::operations::{Failure, Outcome, Step, Steps, UnitPlace};
use crate::value::Filled;

use super::command_line::RunForm;
use super::variables::Variables;

/// What a program inherits from the environment tay runs in when `inherit`
/// does not say.
const INHERITED: &[&str] = &["PATH", "HOME", "USER", "LANG", "TMPDIR"];

/// How often a readiness command is looked at for its end.
const CHECK_LOOK: Duration = Duration::from_millis(50);

#[derive(Debug, thiserror::Error)]
enum HostError {
    #[error("{what} holds a secret, which tay never puts in a command line")]
    SecretInCommandLine { what: &'static str },
    #[error("tay failed while {doing}: {why}")]
    Panicked { doing: &'static str, why: String },
}

/// The files a unit that runs on the host keeps.
pub fn files(place: &UnitPlace) -> UnitFiles {
    UnitFiles::of(&place.environment.layout, &place.unit)
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

/// A program and its arguments, from words with no secret in them, as the
/// planner sees to.
pub fn argv(words: &[Filled], what: &'static str) -> Result<Vec<String>, Failure> {
    words.iter().map(|word| word_of(word, what)).collect()
}

fn word_of(word: &Filled, what: &'static str) -> Result<String, Failure> {
    match word.plain() {
        Some(exact) => Ok(exact.as_text()),
        None => Err(HostError::SecretInCommandLine { what }.into()),
    }
}

/// `run`, `check` or `undo` as a program and its arguments: a list as it
/// is, and `{ shell: "..." }` run by `/bin/sh -c`.
pub fn run_form(form: &RunForm<Filled>, what: &'static str) -> Result<Vec<String>, Failure> {
    match form {
        RunForm::Argv(words) => argv(words, what),
        RunForm::Shell(text) => Ok(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            word_of(text, what)?,
        ]),
    }
}

/// A command line as a person reads it.
pub fn shown(argv: &[String]) -> String {
    format!("`{}`", argv.join(" "))
}

/// How a step that was waited for ended.
pub enum StepEnd {
    /// Its keeper said how.
    Ended(Ended),
    /// Its keeper ended without saying how; what was left of its group has
    /// been stopped.
    Lost,
}

/// Runs `program` as `step` of attempt `attempt` under a keeper, and waits
/// for it to end. When `running` is given, it is told when the program has
/// started: it is the unit's own.
pub async fn run(
    files: &UnitFiles,
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
pub async fn wait(files: &UnitFiles, running: Option<&Steps>) -> Result<StepEnd, Failure> {
    if let Some(steps) = running
        && let Look::Running(record) = files.child_started().await?
    {
        // The engine may have stopped listening; the step goes on.
        let _ = steps.send(Step::Running {
            since: record.begun,
        });
    }
    match files.ended().await? {
        Look::Ended(_, ended) => Ok(StepEnd::Ended(ended)),
        _ => {
            stop_blocking(files.clone()).await?;
            Ok(StepEnd::Lost)
        }
    }
}

/// Stops what of a unit runs on the host, from its place alone: the step
/// under way, and what is left of a step whose keeper was lost.
pub fn stop(place: &UnitPlace) -> Result<(), Failure> {
    Ok(files(place).stop()?)
}

async fn stop_blocking(files: UnitFiles) -> Result<(), Failure> {
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
    format!("{} at {}", ended.exit, clock::utc(ended.at / 1000))
}

/// The outcome of an attempt whose own program ended: a program that could
/// not be started fails the unit, since starting it again would not mend
/// that.
pub fn ending(end: &StepEnd, after: &str) -> Outcome {
    match end {
        StepEnd::Ended(Ended {
            exit: exit @ Exit::NotStarted { .. },
            ..
        }) => Outcome::Failed(format!("its program {exit}")),
        StepEnd::Ended(ended) => Outcome::Ended {
            failed: !ended.exit.succeeded(),
            detail: format!("{}{after}", ended_words(ended)),
        },
        StepEnd::Lost => Outcome::Ended {
            failed: true,
            detail: format!(
                "ended with no record of how{after}: its keeper was lost, and what was left of it was stopped"
            ),
        },
    }
}

/// Runs a readiness command on the host, as a child of the daemon in a
/// process group of its own, for as long as `limit`; why it did not pass.
pub async fn check(program: Program, limit: Duration) -> Result<(), String> {
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
    let mut child = command
        .spawn()
        .map_err(|error| format!("{} cannot be started: {error}", shown(&program.argv)))?;
    let until = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(match status.code() {
                    Some(code) => format!("{} exited with {code}", shown(&program.argv)),
                    None => format!("{} was ended by a signal", shown(&program.argv)),
                });
            }
            Ok(None) if Instant::now() >= until => {
                // The command is the daemon's own child, not yet waited for,
                // so its group is no other's.
                if let Some(group) = rustix::process::Pid::from_raw(child.id() as i32) {
                    let _ =
                        rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
                }
                let _ = child.wait();
                return Err(format!(
                    "{} did not end within {} seconds",
                    shown(&program.argv),
                    limit.as_secs()
                ));
            }
            Ok(None) => tokio::time::sleep(CHECK_LOOK).await,
            Err(error) => {
                return Err(format!(
                    "{} cannot be waited for: {error}",
                    shown(&program.argv)
                ));
            }
        }
    }
}

fn panicked(doing: &'static str, error: &tokio::task::JoinError) -> Failure {
    HostError::Panicked {
        doing,
        why: error.to_string(),
    }
    .into()
}
