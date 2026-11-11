//! What every step of a command runs with, its `check`, its `run` and its
//! `undo` alike: where it runs, as whom, in which folder, and with which
//! variables; and running one step there, on the host under a keeper,
//! inside another unit's running container, or in a container of its own.

use std::path::{Path, PathBuf};

use crate::keeper::Program;
use crate::module::operations::{self, Failure, Steps, Taking, UnitPlace};
use crate::module::running::command_line::{RunForm, filled_run_form};
use crate::module::running::container::{self, CommandStep, ContainerStepEnd};
use crate::module::running::host;
use crate::module::running::mounts::Mount;
use crate::module::running::variables::Variables;
use crate::module::settings::maybe;
use crate::value::Filled;

use super::produced;

/// The steps of a command's attempt, as its record says them, and the step
/// wiping it or its leaving runs.
pub const CHECK: &str = "check";
pub const RUN: &str = "run";
pub const UNDO: &str = "undo";

/// The variable that names the file a command writes its values to.
const VALUES: &str = "TAY_VALUES";

#[derive(Debug, thiserror::Error)]
#[error("its `undo` {0}")]
struct NotUndone(String);

/// What every step of a command runs with, each value a
/// [`Slot`](crate::module::settings::Slot) as written, a
/// [`Value`](crate::value::Value) once resolved, and [`Filled`] once
/// filled.
#[derive(Debug)]
pub struct StepSettings<V> {
    /// The unit inside whose container it runs, with `in`.
    pub inside: Option<String>,
    /// The image of the container made for each step, with `mounts`.
    pub image: Option<V>,
    pub mounts: Vec<Mount<V>>,
    /// The user it runs as in a container.
    pub user: Option<V>,
    /// `None`: the stack's root on the host, the container's own in one.
    pub workdir: Option<V>,
    pub variables: Variables<V>,
    /// On the host, the variables passed through from the environment tay
    /// was started in. `None`: those a program on the host inherits when
    /// `inherit` does not say.
    pub inherit: Option<Vec<String>>,
}

impl<V> StepSettings<V> {
    pub fn map<'a, W>(&'a self, f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<StepSettings<W>> {
        let image = maybe(&self.image, f);
        let mounts: Vec<Option<Mount<W>>> = self.mounts.iter().map(|m| m.map(f)).collect();
        let user = maybe(&self.user, f);
        let workdir = maybe(&self.workdir, f);
        let variables = self.variables.map(f);
        Some(StepSettings {
            inside: self.inside.clone(),
            image: image?,
            mounts: mounts.into_iter().collect::<Option<_>>()?,
            user: user?,
            workdir: workdir?,
            variables: variables?,
            inherit: self.inherit.clone(),
        })
    }
}

/// Where a command's steps run.
pub enum Where<'a> {
    Host,
    /// Inside the running container of this unit.
    Inside(&'a str),
    /// In a container of its own for each step, from this image.
    Own(&'a Filled),
}

/// How a step of a command came out, wherever it ran.
pub enum StepOutcome {
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
    pub fn on_host(end: host::StepEnd) -> StepOutcome {
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

    pub fn in_container(end: ContainerStepEnd) -> StepOutcome {
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

impl StepSettings<Filled> {
    /// Takes each of these settings into `taking`, by its name.
    pub fn take(&self, taking: &mut Taking) {
        if let Some(inside) = &self.inside {
            taking.setting("in", inside);
        }
        if let Some(image) = &self.image {
            taking.value("image", image);
        }
        if !self.mounts.is_empty() {
            let mounts: Vec<_> = self.mounts.iter().map(Mount::revealed).collect();
            taking.setting("mounts", &mounts);
        }
        for (name, value) in [("user", &self.user), ("workdir", &self.workdir)] {
            if let Some(value) = value {
                taking.value(name, value);
            }
        }
        self.variables.take(taking);
        if let Some(inherit) = &self.inherit {
            taking.setting("inherit", inherit);
        }
    }

    pub fn where_it_runs(&self) -> Where<'_> {
        match (&self.inside, &self.image) {
            (Some(unit), _) => Where::Inside(unit),
            (None, Some(image)) => Where::Own(image),
            (None, None) => Where::Host,
        }
    }

    /// Runs `form` as `step` of attempt `attempt`, where the command runs,
    /// fed `stdin`, and waits for it to end. When `running` is given, it is
    /// told when the program has started: it is the command's own.
    pub async fn step(
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
            UNDO => "`undo`",
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

    /// Runs `undo` where the command runs, and waits for it to exit with 0.
    /// It is no attempt of the unit's: it is noted as a first, and the
    /// record it leaves goes with what else is forgotten of the unit once
    /// its work is undone.
    pub async fn undo(&self, place: &UnitPlace, undo: &RunForm<Filled>) -> Result<(), Failure> {
        let words = match self.step(place, 1, UNDO, undo, None, None).await? {
            StepOutcome::Exited { status: 0, .. } => return Ok(()),
            StepOutcome::Exited { words, .. } | StepOutcome::NotStarted(words) => words,
            StepOutcome::Lost => "ended with no record of how: its keeper was lost".to_string(),
            StepOutcome::Gone => host::GONE.to_string(),
        };
        Err(NotUndone(words).into())
    }

    /// The program `argv`, run on the host with the command's environment
    /// in its folder, with `TAY_VALUES` naming the file it writes its values
    /// to.
    pub fn program(
        &self,
        place: &UnitPlace,
        argv: Vec<String>,
        stdin: Option<PathBuf>,
    ) -> Result<Program, Failure> {
        let values = produced::file(place).display().to_string();
        Ok(Program {
            argv,
            env: host::environment(
                &self.variables,
                self.inherit.as_deref(),
                &[(VALUES, values)],
            ),
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
