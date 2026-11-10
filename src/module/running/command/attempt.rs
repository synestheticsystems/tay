//! A command's attempt on the host: its `check`, if it has one, and then its
//! `run`, each under a keeper. A check that exits 0 finds the command done
//! and nothing more runs; 1 finds it not done; anything else cannot tell,
//! and the command fails without running. Once done, every file under
//! `outputs` must exist, and the values it wrote are kept and given.
//!
//! A command killed part-way, or whose daemon was killed during it, is
//! simply not done: the next `tay up` checks again and runs it again.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::keeper::{Ending, Look, Program};
use crate::module::operations::{
    self, Driving, Failure, Found, Outcome, ReadinessCommand, Running, Step, Steps, UnderWay,
    UnitPlace,
};
use crate::module::running::host::{self, StepEnd};
use crate::value::Filled;

use super::CommandSettings;
use super::produced;

/// The steps of a command's attempt, as its record says them.
const CHECK: &str = "check";
const RUN: &str = "run";

/// The variable that names the file a command writes its values to.
const VALUES: &str = "TAY_VALUES";

#[derive(Debug, thiserror::Error)]
#[error("its record says it was at the step `{0}`, which a command does not have")]
struct UnknownStep(String);

/// Whether a command that is done and wrote no values gives those kept when
/// it was last done. A check that finds it done may write none, while a run
/// that writes none gives none.
#[derive(Clone, Copy)]
enum Kept {
    Given,
    NotGiven,
}

/// What follows a check that has ended.
enum AfterCheck {
    /// The command is done.
    Done,
    /// It is not done: `run` runs.
    NotDone,
    /// The attempt ends so.
    Outcome(Outcome),
}

impl Running for CommandSettings<Filled> {
    fn start(self: Arc<Self>, place: UnitPlace, attempt: u32, steps: Steps) -> Driving {
        Box::pin(async move {
            let from = match self.check {
                Some(_) => CHECK,
                None => RUN,
            };
            self.drive(&place, attempt, from, false, &steps)
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
            Look::Ended(record, ended) => {
                let end = StepEnd::Ended(ended);
                let outcome = match record.step.as_str() {
                    CHECK => match self.checked(&end) {
                        AfterCheck::Done => {
                            self.done(place, "done: its check exited with 0", Kept::Given)?
                        }
                        AfterCheck::NotDone => Outcome::Failed(format!(
                            "not done: attempt {} found it not done, and its daemon ended before it ran",
                            record.attempt
                        )),
                        AfterCheck::Outcome(outcome) => outcome,
                    },
                    RUN => self.ran(place, &end, ", while no daemon ran", Kept::Given)?,
                    other => return Err(UnknownStep(other.to_string()).into()),
                };
                Found::Ended {
                    attempt: record.attempt,
                    outcome,
                }
            }
            Look::Lost(record) => Found::Lost {
                attempt: record.attempt,
            },
        })
    }

    fn resume(self: Arc<Self>, place: UnitPlace, under_way: UnderWay, steps: Steps) -> Driving {
        Box::pin(async move {
            let at = match under_way.step.as_str() {
                CHECK => CHECK,
                RUN => RUN,
                other => return Outcome::Failed(UnknownStep(other.to_string()).to_string()),
            };
            self.drive(&place, under_way.attempt, at, true, &steps)
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
                .program(&place, command, None)
                .map_err(|failure| failure.to_string())?;
            host::check(program, limit).await
        })
    }
}

impl CommandSettings<Filled> {
    /// Drives the attempt from the step `from`: started now, or waited for
    /// when it is `under_way`.
    async fn drive(
        &self,
        place: &UnitPlace,
        attempt: u32,
        from: &str,
        under_way: bool,
        steps: &Steps,
    ) -> Result<Outcome, Failure> {
        let files = host::files(place);
        if from == CHECK {
            // The engine has stopped listening only once it has dropped the
            // attempt, which then goes no further.
            let _ = steps.send(Step::Before("checking".to_string()));
            let end = match under_way {
                true => host::wait(&files, None).await?,
                false => {
                    let check = self
                        .check
                        .as_ref()
                        .ok_or_else(|| UnknownStep(CHECK.to_string()))?;
                    produced::clear(place)?;
                    let argv = host::run_form(check, "`check`")?;
                    let program = self.program(place, argv, None)?;
                    host::run(&files, attempt, CHECK, program, None).await?
                }
            };
            match self.checked(&end) {
                AfterCheck::Done => {
                    return self.done(place, "done: its check exited with 0", Kept::Given);
                }
                AfterCheck::NotDone => {}
                AfterCheck::Outcome(outcome) => return Ok(outcome),
            }
        }
        let end = match under_way && from == RUN {
            true => host::wait(&files, Some(steps)).await?,
            false => {
                produced::clear(place)?;
                let argv = host::run_form(&self.run, "`run`")?;
                let stdin = match &self.stdin {
                    Some(stdin) => Some(operations::text(stdin, "stdin")?.into()),
                    None => None,
                };
                let program = self.program(place, argv, stdin)?;
                host::run(&files, attempt, RUN, program, Some(steps)).await?
            }
        };
        self.ran(place, &end, "", Kept::NotGiven)
    }

    /// What follows a check that ended so.
    fn checked(&self, end: &StepEnd) -> AfterCheck {
        match end {
            StepEnd::Ended(ended) => match &ended.how {
                Ending::Code { code: 0 } => AfterCheck::Done,
                Ending::Code { code: 1 } => AfterCheck::NotDone,
                exit @ Ending::NotStarted { .. } => {
                    AfterCheck::Outcome(Outcome::Failed(format!("its check {exit}")))
                }
                _ => AfterCheck::Outcome(Outcome::Failed(format!(
                    "its check {}: whether it is done cannot be told, so `run` did not run",
                    host::ended_words(ended)
                ))),
            },
            StepEnd::Lost => AfterCheck::Outcome(Outcome::Failed(
                "its check ended with no record of how: its keeper was lost".to_string(),
            )),
            StepEnd::Gone => AfterCheck::Outcome(Outcome::Failed(host::GONE.to_string())),
        }
    }

    /// The outcome of an attempt whose run ended so; `after` is said after
    /// how it ended. `kept` is whether the values kept when it was last done
    /// are given if the run wrote none, as they are when a daemon finds a
    /// run done that an earlier one saw end and took the values of.
    fn ran(
        &self,
        place: &UnitPlace,
        end: &StepEnd,
        after: &str,
        kept: Kept,
    ) -> Result<Outcome, Failure> {
        match end {
            StepEnd::Ended(ended) if ended.how.succeeded() => {
                self.done(place, "done: it ran and exited with 0", kept)
            }
            _ => Ok(match host::ending(end, after) {
                Outcome::Ended { failed, detail } => Outcome::Ended {
                    failed,
                    detail: format!("ran and {detail}"),
                },
                outcome => outcome,
            }),
        }
    }

    /// The command is done: every file under `outputs` must exist, and the
    /// values it gives are what it wrote, or else, when `kept` says so,
    /// what was kept when it was done before.
    fn done(&self, place: &UnitPlace, detail: &str, kept: Kept) -> Result<Outcome, Failure> {
        for (name, path) in &self.outputs {
            let path = operations::text(path, "outputs")?;
            if !Path::new(path).exists() {
                return Ok(Outcome::Failed(format!(
                    "{detail}, and `outputs.{}`, `{path}`, does not exist",
                    name.name
                )));
            }
        }
        let values = match (produced::written(place), kept) {
            (false, Kept::Given) if !self.produces.is_empty() => {
                produced::kept(place, &self.produces)
            }
            _ => produced::take(place, &self.produces),
        };
        Ok(match values {
            Ok(values) => Outcome::Done {
                values,
                detail: detail.to_string(),
            },
            Err(why) => Outcome::Failed(format!("{detail}, but {why}")),
        })
    }

    /// The program `argv`, run with the command's environment in its
    /// folder, with `TAY_VALUES` naming the file it writes its values to.
    fn program(
        &self,
        place: &UnitPlace,
        argv: Vec<String>,
        stdin: Option<std::path::PathBuf>,
    ) -> Result<Program, Failure> {
        let values = produced::file(place).display().to_string();
        Ok(Program {
            argv,
            env: host::environment(&self.variables, None, &[(VALUES, values)]),
            workdir: host::workdir(self.workdir.as_ref(), place)?,
            stdin,
        })
    }
}
