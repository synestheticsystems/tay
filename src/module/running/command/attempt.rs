//! A command's attempt: its `check`, if it has one, and then its `run`,
//! wherever it runs: each step on the host under a keeper, inside another
//! unit's running container, or in a container of its own. A check that
//! exits 0 finds the command done and nothing more runs; 1 finds it not
//! done; anything else cannot tell, and the command fails without running.
//! Once done, every file under `outputs` must exist, the values it wrote
//! are kept and given, and what its `undo` needs is kept when it is to run
//! once the unit leaves the plan.
//!
//! A command killed part-way, or whose daemon was killed during it, is
//! simply not done: the next `tay up` checks again and runs it again. One on
//! the host, or in a container of its own, is found again under way by the
//! next daemon; one inside another unit's container leaves nothing to be
//! found by, so is simply not done. A step that has ended, wherever it ran,
//! is found again as it ended.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::keeper::Look;
use crate::module::operations::{
    self, Driving, Failure, Finding, Fingerprint, Fingerprinting, Found, Outcome, ReadinessCommand,
    Running, Step, Steps, Taking, UnderWay, UnitPlace,
};
use crate::module::running::container::{self, FoundStep};
use crate::module::running::host;
use crate::value::Filled;

use super::step::{CHECK, RUN, StepOutcome, Where};
use super::{CommandSettings, WhenRemoved, kept_undo, produced};

#[derive(Debug, thiserror::Error)]
enum CommandError {
    #[error("its record says it was at the step `{0}`, which a command does not have")]
    UnknownStep(String),
    #[error("a command run inside another unit's container is not found again under way")]
    NotFoundInside,
}

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

    fn found(self: Arc<Self>, place: UnitPlace) -> Finding {
        Box::pin(async move { self.found_now(&place).await })
    }

    fn resume(self: Arc<Self>, place: UnitPlace, under_way: UnderWay, steps: Steps) -> Driving {
        Box::pin(async move {
            let at = match under_way.step.as_str() {
                CHECK => CHECK,
                RUN => RUN,
                other => {
                    return Outcome::Failed(
                        CommandError::UnknownStep(other.to_string()).to_string(),
                    );
                }
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
            match self.steps.where_it_runs() {
                Where::Host => {
                    let program = self
                        .steps
                        .program(&place, command, None)
                        .map_err(|failure| failure.to_string())?;
                    host::check(program, limit).await
                }
                Where::Inside(unit) => {
                    container::check_inside(&place.of(unit), &command, limit).await
                }
                Where::Own(_) => container::check_inside(&place, &command, limit).await,
            }
        })
    }

    fn started_from(self: Arc<Self>, place: UnitPlace) -> Fingerprinting {
        Box::pin(async move { Ok(self.fingerprint(&place)) })
    }
}

impl CommandSettings<Filled> {
    /// What the command is started from, by the settings its steps run
    /// with. What wiping it or its leaving the plan runs is not among
    /// them.
    fn fingerprint(&self, place: &UnitPlace) -> Fingerprint {
        let mut taking = Taking::of(place);
        taking.setting("run", &self.run.revealed());
        if let Some(check) = &self.check {
            taking.setting("check", &check.revealed());
        }
        self.steps.take(&mut taking);
        if let Some(stdin) = &self.stdin {
            taking.value("stdin", stdin);
        }
        let produces: Vec<(&str, bool)> = (self.produces.iter())
            .map(|(name, secret)| (name.name.as_str(), *secret))
            .collect();
        if !produces.is_empty() {
            taking.setting("produces", &produces);
        }
        for (name, path) in &self.outputs {
            taking.value(format!("outputs.{}", name.name), path);
        }
        taking.taken()
    }

    /// The command as a daemon that did not start it finds it.
    async fn found_now(&self, place: &UnitPlace) -> Result<Found, Failure> {
        let found = match self.steps.where_it_runs() {
            Where::Host => {
                return match host::files(place).look()? {
                    Look::Never => Ok(Found::Never),
                    Look::Running(record) => Ok(Found::UnderWay(UnderWay {
                        attempt: record.attempt,
                        step: record.step,
                    })),
                    Look::Lost(record) => Ok(Found::Lost {
                        attempt: record.attempt,
                    }),
                    Look::Ended(record, ended) => self.found_ended(
                        place,
                        record.attempt,
                        &record.step,
                        &StepOutcome::on_host(host::StepEnd::Ended(ended)),
                    ),
                };
            }
            Where::Inside(_) => container::found_inside(place)?,
            Where::Own(_) => container::found_own(place, &produced::file(place)).await?,
        };
        match found {
            FoundStep::Never => Ok(Found::Never),
            FoundStep::UnderWay { attempt, step } => {
                Ok(Found::UnderWay(UnderWay { attempt, step }))
            }
            FoundStep::Ended {
                attempt,
                step,
                ended,
            } => self.found_ended(place, attempt, &step, &StepOutcome::in_container(ended)),
        }
    }

    /// The command as a daemon that did not start it finds it, when step
    /// `step` of attempt `attempt` was the last, and ended so.
    fn found_ended(
        &self,
        place: &UnitPlace,
        attempt: u32,
        step: &str,
        end: &StepOutcome,
    ) -> Result<Found, Failure> {
        let outcome = match step {
            CHECK => match self.checked(end) {
                AfterCheck::Done => {
                    self.done(place, "done: its check exited with 0", Kept::Given)?
                }
                AfterCheck::NotDone => Outcome::Failed(format!(
                    "not done: attempt {attempt} found it not done, and its daemon ended before it ran"
                )),
                AfterCheck::Outcome(outcome) => outcome,
            },
            RUN => self.ran(place, end, ", while no daemon ran", Kept::Given)?,
            other => return Err(CommandError::UnknownStep(other.to_string()).into()),
        };
        Ok(Found::Ended { attempt, outcome })
    }

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
        // The engine keeps what it was started from, unless it has stopped
        // listening, which it does only once it has dropped the attempt,
        // which then goes no further.
        if !under_way {
            let _ = steps.send(Step::Started(self.fingerprint(place)));
        }
        if from == CHECK {
            let _ = steps.send(Step::Before("checking".to_string()));
            let end = match under_way {
                true => self.wait(place, attempt, CHECK, None).await?,
                false => {
                    let check = self
                        .check
                        .as_ref()
                        .ok_or_else(|| CommandError::UnknownStep(CHECK.to_string()))?;
                    produced::clear(place)?;
                    (self.steps)
                        .step(place, attempt, CHECK, check, None, None)
                        .await?
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
            true => self.wait(place, attempt, RUN, Some(steps)).await?,
            false => {
                produced::clear(place)?;
                let stdin = match &self.stdin {
                    Some(stdin) => Some(PathBuf::from(operations::text(stdin, "stdin")?)),
                    None => None,
                };
                (self.steps)
                    .step(place, attempt, RUN, &self.run, stdin, Some(steps))
                    .await?
            }
        };
        self.ran(place, &end, "", Kept::NotGiven)
    }

    /// Waits for step `step` of attempt `attempt`, under way, which a daemon
    /// that did not start it found, to end.
    async fn wait(
        &self,
        place: &UnitPlace,
        attempt: u32,
        step: &str,
        running: Option<&Steps>,
    ) -> Result<StepOutcome, Failure> {
        Ok(match self.steps.where_it_runs() {
            Where::Host => StepOutcome::on_host(host::wait(&host::files(place), running).await?),
            Where::Own(_) => StepOutcome::in_container(
                container::wait_own(place, attempt, step, &produced::file(place), running).await?,
            ),
            Where::Inside(_) => return Err(CommandError::NotFoundInside.into()),
        })
    }

    /// What follows a check that ended so.
    fn checked(&self, end: &StepOutcome) -> AfterCheck {
        match end {
            StepOutcome::Exited { status: 0, .. } => AfterCheck::Done,
            StepOutcome::Exited { status: 1, .. } => AfterCheck::NotDone,
            StepOutcome::NotStarted(words) => {
                AfterCheck::Outcome(Outcome::Failed(format!("its check {words}")))
            }
            StepOutcome::Exited { words, .. } => AfterCheck::Outcome(Outcome::Failed(format!(
                "its check {words}: whether it is done cannot be told, so `run` did not run"
            ))),
            StepOutcome::Lost => AfterCheck::Outcome(Outcome::Failed(
                "its check ended with no record of how: its keeper was lost".to_string(),
            )),
            StepOutcome::Gone => AfterCheck::Outcome(Outcome::Failed(host::GONE.to_string())),
        }
    }

    /// The outcome of an attempt whose run ended so; `after` is said after
    /// how it ended. `kept` is whether the values kept when it was last done
    /// are given if the run wrote none, as they are when a daemon finds a
    /// run done that an earlier one saw end and took the values of.
    fn ran(
        &self,
        place: &UnitPlace,
        end: &StepOutcome,
        after: &str,
        kept: Kept,
    ) -> Result<Outcome, Failure> {
        Ok(match end {
            StepOutcome::Exited { status: 0, .. } => {
                return self.done(place, "done: it ran and exited with 0", kept);
            }
            StepOutcome::Exited { status, words } => Outcome::Ended {
                failed: true,
                detail: format!("ran and {words}{after}"),
                exit_status: Some(*status),
            },
            StepOutcome::NotStarted(words) => Outcome::Failed(format!("its program {words}")),
            StepOutcome::Lost => Outcome::Ended {
                failed: true,
                detail: format!(
                    "ran and ended with no record of how{after}: its keeper was lost, and what was left of it was stopped"
                ),
                exit_status: None,
            },
            StepOutcome::Gone => Outcome::Failed(host::GONE.to_string()),
        })
    }

    /// The command is done: every file under `outputs` must exist, the
    /// values it gives are what it wrote, or else, when `kept` says so and
    /// it keeps its values, what was kept when it was done before, and what
    /// its `undo` needs is kept when it is to run once the unit leaves the
    /// plan.
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
            (false, Kept::Given) if self.keeps_values && !self.produces.is_empty() => {
                produced::kept(place, &self.produces)
            }
            _ => produced::take(place, &self.produces, self.keeps_values),
        };
        let values = match values {
            Ok(values) => values,
            Err(why) => return Ok(Outcome::Failed(format!("{detail}, but {why}"))),
        };
        Ok(match self.keep_undo(place) {
            Ok(()) => Outcome::Done {
                values,
                detail: detail.to_string(),
            },
            Err(why) => Outcome::Failed(format!(
                "{detail}, but what its `undo` needs could not be kept: {why}"
            )),
        })
    }

    /// Keeps what its `undo` needs, as it ran now and with the units that
    /// run that it needed, when the `undo` is to run once the unit leaves
    /// the plan; forgets what was kept for an earlier one otherwise, as
    /// when `when_removed` was changed to `keep`.
    fn keep_undo(&self, place: &UnitPlace) -> Result<(), Failure> {
        match (&self.undo, self.when_removed) {
            (Some(undo), WhenRemoved::Undo) => {
                kept_undo::keep(place, undo, &self.steps, &self.needs_that_run)
            }
            _ => kept_undo::forget(place),
        }
    }
}
