//! A process's attempt: its `prepare` commands, each to its end, before the
//! first attempt each time it is brought up, and then its command, each run
//! under a keeper.

use std::sync::Arc;
use std::time::Duration;

use crate::keeper::{Look, Program};
use crate::module::operations::{
    Driving, Failure, Found, Outcome, ReadinessCommand, Running, Step, Steps, UnderWay, UnitPlace,
};
use crate::module::running::host::{self, StepEnd};
use crate::value::Filled;

use super::ProcessSettings;

/// The step of an attempt that runs the process's command; each `prepare`
/// command is the step `prepare N`, N counting from 1.
const COMMAND: &str = "command";

#[derive(Debug, thiserror::Error)]
#[error("its record says it was at the step `{0}`, which a process does not have")]
struct UnknownStep(String);

/// Where an attempt is driven from.
enum From {
    /// Its first `prepare` command, when it prepares, or else its command.
    Start { prepare: bool },
    /// The step under way, waited for, and then what follows it.
    UnderWay(String),
}

impl Running for ProcessSettings<Filled> {
    fn start(self: Arc<Self>, place: UnitPlace, attempt: u32, steps: Steps) -> Driving {
        // A process prepares before its first attempt each time it is
        // brought up, and not before a restart.
        let prepare = attempt == 1;
        Box::pin(async move {
            self.drive(&place, attempt, From::Start { prepare }, &steps)
                .await
                .unwrap_or_else(|failure| Outcome::Failed(failure.to_string()))
        })
    }

    fn found(&self, place: &UnitPlace) -> Result<Found, Failure> {
        Ok(match host::files(place).look()? {
            Look::Never => Found::Never,
            Look::Running(record) => Found::UnderWay(UnderWay {
                attempt: record.attempt,
                step: record.step,
            }),
            Look::Ended(record, ended) => Found::Ended {
                attempt: record.attempt,
                outcome: match record.step.as_str() {
                    COMMAND => host::ending(&StepEnd::Ended(ended), ", while no daemon ran"),
                    // Nothing went on with the attempt once its daemon had
                    // gone, and a restart would not prepare.
                    _ => Outcome::Failed(format!(
                        "attempt {} was preparing when its daemon ended, and was not started",
                        record.attempt
                    )),
                },
            },
            Look::Lost(record) => Found::Lost {
                attempt: record.attempt,
            },
        })
    }

    fn resume(self: Arc<Self>, place: UnitPlace, under_way: UnderWay, steps: Steps) -> Driving {
        Box::pin(async move {
            let from = From::UnderWay(under_way.step);
            self.drive(&place, under_way.attempt, from, &steps)
                .await
                .unwrap_or_else(|failure| Outcome::Failed(failure.to_string()))
        })
    }

    fn readiness_command(
        self: Arc<Self>,
        place: UnitPlace,
        command: Vec<String>,
        limit: Duration,
    ) -> ReadinessCommand {
        Box::pin(async move {
            let program = self
                .program(&place, command)
                .map_err(|failure| failure.to_string())?;
            host::check(program, limit).await
        })
    }
}

impl ProcessSettings<Filled> {
    async fn drive(
        &self,
        place: &UnitPlace,
        attempt: u32,
        from: From,
        steps: &Steps,
    ) -> Result<Outcome, Failure> {
        let files = host::files(place);
        let first = match from {
            From::Start { prepare: true } => 0,
            From::Start { prepare: false } => self.prepare.len(),
            From::UnderWay(step) if step == COMMAND => {
                let end = host::wait(&files, Some(steps)).await?;
                return Ok(host::ending(&end, ""));
            }
            From::UnderWay(step) => {
                let index = prepare_index(&step)?;
                // The engine has stopped listening only once it has dropped
                // the attempt, which then goes no further.
                let _ = steps.send(Step::Before(self.preparing(index)?));
                let end = host::wait(&files, None).await?;
                if let Some(failed) = self.prepare_failed(index, &end)? {
                    return Ok(failed);
                }
                index + 1
            }
        };
        for index in first..self.prepare.len() {
            // The engine has stopped listening only once it has dropped
            // the attempt, which then goes no further.
            let _ = steps.send(Step::Before(self.preparing(index)?));
            let argv = host::argv(&self.prepare[index], "a `prepare` command")?;
            let program = self.program(place, argv)?;
            let end = host::run(&files, attempt, &prepare_step(index), program, None).await?;
            if let Some(failed) = self.prepare_failed(index, &end)? {
                return Ok(failed);
            }
        }
        let argv = host::argv(&self.command, "`command`")?;
        let program = self.program(place, argv)?;
        let end = host::run(&files, attempt, COMMAND, program, Some(steps)).await?;
        Ok(host::ending(&end, ""))
    }

    /// The program `argv`, run with the process's environment in its
    /// folder.
    fn program(&self, place: &UnitPlace, argv: Vec<String>) -> Result<Program, Failure> {
        Ok(Program {
            argv,
            env: host::environment(&self.variables, self.inherit.as_deref(), &[]),
            workdir: host::workdir(self.workdir.as_ref(), place)?,
            stdin: None,
        })
    }

    /// What the attempt does while it runs its `index`th `prepare` command.
    fn preparing(&self, index: usize) -> Result<String, Failure> {
        let words = self
            .prepare
            .get(index)
            .ok_or_else(|| UnknownStep(prepare_step(index)))?;
        let argv = host::argv(words, "a `prepare` command")?;
        Ok(format!("preparing: {}", host::shown(&argv)))
    }

    /// The outcome of the attempt when its `index`th `prepare` command did
    /// not exit with 0: it cannot start until something changes.
    fn prepare_failed(&self, index: usize, end: &StepEnd) -> Result<Option<Outcome>, Failure> {
        let argv = host::argv(&self.prepare[index], "a `prepare` command")?;
        let shown = host::shown(&argv);
        Ok(match end {
            StepEnd::Ended(ended) if ended.how.succeeded() => None,
            StepEnd::Ended(ended) => Some(Outcome::Failed(format!(
                "{shown}, which it prepares with, {}",
                host::ended_words(ended)
            ))),
            StepEnd::Lost => Some(Outcome::Failed(format!(
                "{shown}, which it prepares with, ended with no record of how: its keeper was lost"
            ))),
            StepEnd::Gone => Some(Outcome::Failed(host::GONE.to_string())),
        })
    }
}

fn prepare_step(index: usize) -> String {
    format!("prepare {}", index + 1)
}

fn prepare_index(step: &str) -> Result<usize, UnknownStep> {
    step.strip_prefix("prepare ")
        .and_then(|number| number.parse::<usize>().ok())
        .and_then(|number| number.checked_sub(1))
        .ok_or_else(|| UnknownStep(step.to_string()))
}
