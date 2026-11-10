//! A command's attempt: its `check`, if it has one, and then its `run`,
//! wherever it runs: each step on the host under a keeper, inside another
//! unit's running container, or in a container of its own. A check that
//! exits 0 finds the command done and nothing more runs; 1 finds it not
//! done; anything else cannot tell, and the command fails without running.
//! Once done, every file under `outputs` must exist, and the values it wrote
//! are kept and given.
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

use crate::keeper::{Look, Program};
use crate::module::operations::{
    self, Comparing, Driving, Failure, Finding, Found, Outcome, ReadinessCommand, Running, Step,
    Steps, UnderWay, UnitPlace,
};
use crate::module::running::command_line::{RunForm, filled_run_form};
use crate::module::running::container::{self, CommandStep, ContainerStepEnd, FoundStep};
use crate::module::running::host;
use crate::value::Filled;

use super::CommandSettings;
use super::produced;

/// The steps of a command's attempt, as its record says them.
const CHECK: &str = "check";
const RUN: &str = "run";

/// The variable that names the file a command writes its values to.
const VALUES: &str = "TAY_VALUES";

#[derive(Debug, thiserror::Error)]
enum CommandError {
    #[error("its record says it was at the step `{0}`, which a command does not have")]
    UnknownStep(String),
    #[error("a command run inside another unit's container is not found again under way")]
    NotFoundInside,
}

/// Where a command's steps run.
enum Where<'a> {
    Host,
    /// Inside the running container of this unit.
    Inside(&'a str),
    /// In a container of its own for each step, from this image.
    Own(&'a Filled),
}

/// How a step of a command came out, wherever it ran.
enum StepOutcome {
    /// Its program ended, as these words say, read after "its check" or
    /// "ran and", with this exit status, as a shell gives it: the code it
    /// exited with, or 128 and the signal that ended it.
    Exited { status: i64, words: String },
    /// Its program could not be started, as these words say.
    NotStarted(String),
    /// Its keeper ended without saying how; what was left of its group has
    /// been stopped.
    Lost,
    /// Its record went with the environment's folder; what was left of its
    /// group has been stopped.
    Gone,
}

impl StepOutcome {
    fn on_host(end: host::StepEnd) -> StepOutcome {
        match end {
            host::StepEnd::Ended(ended) => match ended.how.exit_status() {
                Some(status) => StepOutcome::Exited {
                    status,
                    words: host::ended_words(&ended),
                },
                None => StepOutcome::NotStarted(ended.how.to_string()),
            },
            host::StepEnd::Lost => StepOutcome::Lost,
            host::StepEnd::Gone => StepOutcome::Gone,
        }
    }

    fn in_container(end: ContainerStepEnd) -> StepOutcome {
        match end {
            ContainerStepEnd::Exited { code, words } => StepOutcome::Exited {
                status: code,
                words,
            },
            ContainerStepEnd::NotStarted(why) => {
                StepOutcome::NotStarted(format!("could not be started: {why}"))
            }
        }
    }
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
            match self.where_it_runs() {
                Where::Host => {
                    let program = self
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

    /// A command runs again at each `tay up` whose check finds it not done,
    /// whatever its settings were, so none is replaced.
    fn outdated(self: Arc<Self>, _: UnitPlace) -> Comparing {
        Box::pin(async { Ok(false) })
    }
}

impl CommandSettings<Filled> {
    fn where_it_runs(&self) -> Where<'_> {
        match (&self.inside, &self.image) {
            (Some(unit), _) => Where::Inside(unit),
            (None, Some(image)) => Where::Own(image),
            (None, None) => Where::Host,
        }
    }

    /// The command as a daemon that did not start it finds it.
    async fn found_now(&self, place: &UnitPlace) -> Result<Found, Failure> {
        let found = match self.where_it_runs() {
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
        if from == CHECK {
            // The engine has stopped listening only once it has dropped the
            // attempt, which then goes no further.
            let _ = steps.send(Step::Before("checking".to_string()));
            let end = match under_way {
                true => self.wait(place, attempt, CHECK, None).await?,
                false => {
                    let check = self
                        .check
                        .as_ref()
                        .ok_or_else(|| CommandError::UnknownStep(CHECK.to_string()))?;
                    produced::clear(place)?;
                    self.step(place, attempt, CHECK, check, None, None).await?
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
                self.step(place, attempt, RUN, &self.run, stdin, Some(steps))
                    .await?
            }
        };
        self.ran(place, &end, "", Kept::NotGiven)
    }

    /// Runs `form` as `step` of attempt `attempt`, where the command runs,
    /// fed `stdin`, and waits for it to end. When `running` is given, it is
    /// told when the program has started: it is the command's own.
    async fn step(
        &self,
        place: &UnitPlace,
        attempt: u32,
        step: &'static str,
        form: &RunForm<Filled>,
        stdin: Option<PathBuf>,
        running: Option<&Steps>,
    ) -> Result<StepOutcome, Failure> {
        let what = match step {
            CHECK => "`check`",
            _ => "`run`",
        };
        let argv = filled_run_form(form, what)?;
        let values = produced::file(place);
        Ok(match self.where_it_runs() {
            Where::Host => {
                let program = self.program(place, argv, stdin)?;
                let files = host::files(place);
                StepOutcome::on_host(host::run(&files, attempt, step, program, running).await?)
            }
            Where::Inside(unit) => {
                let step = self.in_container(attempt, step, argv, stdin.as_deref())?;
                let ended = container::run_inside(place, unit, step, &values, running);
                StepOutcome::in_container(ended.await?)
            }
            Where::Own(image) => {
                let step = self.in_container(attempt, step, argv, stdin.as_deref())?;
                let image = operations::text(image, "image")?;
                let ended = container::run_own(place, image, &self.mounts, step, &values, running);
                StepOutcome::in_container(ended.await?)
            }
        })
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
        Ok(match self.where_it_runs() {
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

    /// The command is done: every file under `outputs` must exist, and the
    /// values it gives are what it wrote, or else, when `kept` says so and
    /// it keeps its values, what was kept when it was done before.
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
        Ok(match values {
            Ok(values) => Outcome::Done {
                values,
                detail: detail.to_string(),
            },
            Err(why) => Outcome::Failed(format!("{detail}, but {why}")),
        })
    }

    /// The program `argv`, run on the host with the command's environment
    /// in its folder, with `TAY_VALUES` naming the file it writes its values
    /// to.
    fn program(
        &self,
        place: &UnitPlace,
        argv: Vec<String>,
        stdin: Option<PathBuf>,
    ) -> Result<Program, Failure> {
        let values = produced::file(place).display().to_string();
        Ok(Program {
            argv,
            env: host::environment(&self.variables, None, &[(VALUES, values)]),
            workdir: host::workdir(self.workdir.as_ref(), place)?,
            stdin,
        })
    }

    /// The step `step` of attempt `attempt`, the program `argv` fed the
    /// file at `stdin`, as it is run in a container: with only the
    /// environment the definition gives it, as `user` in `workdir` when
    /// they are given.
    fn in_container(
        &self,
        attempt: u32,
        step: &'static str,
        argv: Vec<String>,
        stdin: Option<&Path>,
    ) -> Result<CommandStep, Failure> {
        let mut env = Vec::new();
        let mut secrets = Vec::new();
        for (name, value) in &self.variables.env {
            env.push(format!("{name}={}", value.reveal()));
        }
        for (name, value) in &self.variables.secret_env {
            env.push(format!("{name}={}", value.reveal()));
            secrets.push(value.reveal());
        }
        let stdin = match stdin {
            Some(path) => {
                Some(std::fs::read(path).map_err(crate::disk::DiskError::io(path, "read"))?)
            }
            None => None,
        };
        Ok(CommandStep {
            attempt,
            step,
            argv,
            env,
            secrets,
            user: self
                .user
                .as_ref()
                .map(|user| operations::scalar(user, "user"))
                .transpose()?,
            workdir: self
                .workdir
                .as_ref()
                .map(|workdir| Ok::<_, Failure>(operations::text(workdir, "workdir")?.to_string()))
                .transpose()?,
            stdin,
        })
    }
}
