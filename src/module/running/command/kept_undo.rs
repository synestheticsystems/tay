//! What a command with `when_removed: undo` keeps once it is done, so that
//! its `undo` can run when the unit is no longer in the plan, with what the
//! unit last ran with: the `undo`, where it runs, and its environment. A
//! variable that holds a secret is kept with the secrets, readable by this
//! account only; the rest in the unit's own folder. It is found and run by
//! the unit's place alone, whatever the plan holds, so that whatever comes
//! to run it, a plan that no longer has the unit or anything else, finds
//! it.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::produced;
use super::step::{StepSettings, Where};
use crate::disk::{DiskError, Environment, remove_if_there, write_secret, write_whole};
use crate::module::operations::{self, Failure, UnitPlace};
use crate::module::running::command_line::{RunForm, filled_run_form};
use crate::module::running::mounts::{KeptMount, Mount};
use crate::module::running::remove_record;
use crate::module::running::variables::Variables;
use crate::value::Filled;

/// The file it is kept in, in the unit's own folder.
const KEPT: &str = "undo.json";

/// What the file of its secrets is named, after the unit's name, in the
/// secrets folder: a unit's name has no dot, so it is no unit's own.
const SECRETS: &str = "undo";

#[derive(Debug, thiserror::Error)]
enum KeptUndoError {
    #[error("nothing is kept of what its `undo` needs")]
    NotKept,
    #[error("what its `undo` needs, kept in `{}`, is not in a form this tay reads", .0.display())]
    Unread(PathBuf),
    #[error("what its `undo` needs is kept without the secret in `{0}`, which it runs with")]
    NoSecret(String),
}

/// What an `undo` needs, but for the values of the variables that hold a
/// secret.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Kept {
    /// The program and its arguments.
    undo: Vec<String>,
    runs: RunsIn,
    user: Option<String>,
    workdir: Option<String>,
    env: Vec<(String, String)>,
    /// The names of the variables that hold a secret.
    secret_env: Vec<String>,
}

/// Where an `undo` runs.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RunsIn {
    Host,
    /// Inside the running container of this unit.
    Inside(String),
    /// In a container of its own, from this image, with these mounts.
    Own {
        image: String,
        mounts: Vec<KeptMount>,
    },
}

/// The values of the variables that hold a secret, by name.
type Secrets = BTreeMap<String, String>;

impl Kept {
    /// What `undo` needs, run as `steps` say, and the values of the
    /// variables that hold a secret.
    fn of(
        undo: &RunForm<Filled>,
        steps: &StepSettings<Filled>,
    ) -> Result<(Kept, Secrets), Failure> {
        let runs = match steps.where_it_runs() {
            Where::Host => RunsIn::Host,
            Where::Inside(unit) => RunsIn::Inside(unit.to_string()),
            Where::Own(image) => RunsIn::Own {
                image: operations::text(image, "image")?.to_string(),
                mounts: (steps.mounts.iter())
                    .map(Mount::kept)
                    .collect::<Result<_, _>>()?,
            },
        };
        let env = (steps.variables.env.iter())
            .map(|(name, value)| Ok((name.clone(), operations::scalar(value, "env")?)))
            .collect::<Result<_, Failure>>()?;
        let secrets: Secrets = (steps.variables.secret_env.iter())
            .map(|(name, value)| (name.clone(), value.reveal()))
            .collect();
        let kept = Kept {
            undo: filled_run_form(undo, "`undo`")?,
            runs,
            user: (steps.user.as_ref())
                .map(|user| operations::scalar(user, "user"))
                .transpose()?,
            workdir: (steps.workdir.as_ref())
                .map(|workdir| operations::text(workdir, "workdir").map(str::to_string))
                .transpose()?,
            env,
            secret_env: secrets.keys().cloned().collect(),
        };
        Ok((kept, secrets))
    }

    /// The `undo`, and what its step runs with, as they were filled when
    /// they were kept, with `secrets`.
    fn filled(
        &self,
        secrets: &Secrets,
    ) -> Result<(RunForm<Filled>, StepSettings<Filled>), KeptUndoError> {
        let secret_env = (self.secret_env.iter())
            .map(|name| match secrets.get(name) {
                Some(value) => Ok((name.clone(), Filled::secret(value))),
                None => Err(KeptUndoError::NoSecret(name.clone())),
            })
            .collect::<Result<_, _>>()?;
        let (inside, image, mounts) = match &self.runs {
            RunsIn::Host => (None, None, Vec::new()),
            RunsIn::Inside(unit) => (Some(unit.clone()), None, Vec::new()),
            RunsIn::Own { image, mounts } => (
                None,
                Some(Filled::text(image)),
                mounts.iter().map(KeptMount::filled).collect(),
            ),
        };
        let steps = StepSettings {
            inside,
            image,
            mounts,
            user: self.user.as_ref().map(Filled::text),
            workdir: self.workdir.as_ref().map(Filled::text),
            variables: Variables {
                env: (self.env.iter())
                    .map(|(name, value)| (name.clone(), Filled::text(value)))
                    .collect(),
                secret_env,
            },
        };
        let undo = RunForm::Argv(self.undo.iter().map(Filled::text).collect());
        Ok((undo, steps))
    }
}

/// Keeps what `undo` needs, run as `steps` say, for the unit at `place`, as
/// it is filled now, in place of what was kept before.
pub fn keep(
    place: &UnitPlace,
    undo: &RunForm<Filled>,
    steps: &StepSettings<Filled>,
) -> Result<(), Failure> {
    let (kept, secrets) = Kept::of(undo, steps)?;
    // The secrets go first, so that what is kept never names one that is
    // not.
    let secrets_path = secrets_path(place);
    match secrets.is_empty() {
        true => remove_if_there(&secrets_path)?,
        false => write_secret(&secrets_path, json(&secrets).as_bytes())?,
    }
    let path = kept_path(place);
    // A command that runs in a container has nothing of its own on the
    // host until it keeps something.
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(DiskError::io(folder, "made"))?;
    }
    Ok(write_whole(&path, json(&kept).as_bytes(), 0o644)?)
}

/// Forgets what was kept for the `undo` of the unit at `place`, if
/// anything was.
pub fn forget(place: &UnitPlace) -> Result<(), Failure> {
    remove_if_there(&kept_path(place))?;
    Ok(remove_if_there(&secrets_path(place))?)
}

/// The units of `environment` for which what an `undo` needs is kept, the
/// last kept first.
pub fn units(environment: &Environment) -> Result<Vec<String>, Failure> {
    let folder = environment.layout.units_state();
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(DiskError::io(folder, "read")(error).into()),
    };
    let mut kept: Vec<(SystemTime, String)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(DiskError::io(&folder, "read"))?;
        let path = entry.path().join(KEPT);
        let when = match std::fs::metadata(&path).and_then(|metadata| metadata.modified()) {
            Ok(when) => when,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(DiskError::io(path, "read")(error).into()),
        };
        if let Some(unit) = entry.file_name().to_str() {
            kept.push((when, unit.to_string()));
        }
    }
    kept.sort();
    Ok(kept.into_iter().rev().map(|(_, unit)| unit).collect())
}

/// Runs the `undo` kept for the unit at `place`, where it ran and with what
/// it last ran with, and once it has run, forgets the unit: what was kept
/// for its `undo`, the values it produced, and its record.
pub async fn run(place: &UnitPlace) -> Result<(), Failure> {
    let kept: Kept = read(&kept_path(place))?.ok_or(KeptUndoError::NotKept)?;
    let secrets: Secrets = read(&secrets_path(place))?.unwrap_or_default();
    let (undo, steps) = kept.filled(&secrets)?;
    steps.undo(place, &undo).await?;
    forget(place)?;
    produced::forget(place)?;
    remove_record(place)
}

fn kept_path(place: &UnitPlace) -> PathBuf {
    place.environment.layout.unit_state(&place.unit).join(KEPT)
}

fn secrets_path(place: &UnitPlace) -> PathBuf {
    place
        .environment
        .layout
        .secrets()
        .join(format!("{}.{SECRETS}", place.unit))
}

fn json(value: &impl Serialize) -> String {
    let text = serde_json::to_string_pretty(value).expect("what is kept serializes");
    format!("{text}\n")
}

/// What is kept at `path`; `None` when nothing is. What the reader says of
/// one it cannot read is not given: it may quote a secret.
fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, Failure> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DiskError::io(path, "read")(error).into()),
    };
    match serde_json::from_str(&text) {
        Ok(kept) => Ok(Some(kept)),
        Err(_) => Err(KeptUndoError::Unread(path.to_path_buf()).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::running::mounts::{Source, UnitVolume};
    use crate::value::Exact;

    /// The settings of a step run in a container of its own, with a host
    /// folder and another unit's volume mounted, a variable, and a secret.
    fn own() -> StepSettings<Filled> {
        let mount = |source, readonly: Option<bool>| Mount {
            source,
            to: Filled::text("/data"),
            readonly: readonly.map(|readonly| Filled::of(Exact::Bool(readonly), false)),
        };
        StepSettings {
            inside: None,
            image: Some(Filled::text("alpine:3")),
            mounts: vec![
                mount(Source::Host(Filled::text("/srv/data")), None),
                mount(
                    Source::Volume(UnitVolume {
                        unit: "db".to_string(),
                        volume: "data".to_string(),
                    }),
                    Some(false),
                ),
            ],
            user: Some(Filled::number(999)),
            workdir: Some(Filled::text("/work")),
            variables: Variables {
                env: vec![("MODE".to_string(), Filled::text("fast"))],
                secret_env: vec![("TOKEN".to_string(), Filled::secret("hunter2"))],
            },
        }
    }

    #[test]
    fn what_an_undo_needs_is_had_again_as_it_was_kept_and_its_secrets_apart() {
        let undo = RunForm::Shell(Filled::text("rm -rf /data/*"));
        let steps = own();
        let (kept, secrets) = Kept::of(&undo, &steps).unwrap();
        let written = json(&kept);
        assert!(!written.contains("hunter2"), "{written}");
        assert_eq!(secrets["TOKEN"], "hunter2");
        let read: Kept = serde_json::from_str(&written).unwrap();
        assert_eq!(read, kept);
        let (again, filled) = read.filled(&secrets).unwrap();
        assert_eq!(
            filled_run_form(&again, "`undo`").unwrap(),
            ["/bin/sh", "-c", "rm -rf /data/*"]
        );
        // What it is had again as runs where it ran, with what it ran with.
        assert_eq!(Kept::of(&again, &filled).unwrap(), (kept, secrets));
        assert!(
            matches!(filled.where_it_runs(), Where::Own(image) if image.reveal() == "alpine:3")
        );
        assert_eq!(filled.variables.secret_env[0].1.reveal(), "hunter2");
        assert!(filled.variables.secret_env[0].1.is_secret());
    }

    #[test]
    fn a_secret_that_is_not_kept_is_said_by_name() {
        let (kept, _) = Kept::of(&RunForm::Argv(vec![Filled::text("true")]), &own()).unwrap();
        let missing = kept.filled(&Secrets::new()).unwrap_err().to_string();
        assert_eq!(
            missing,
            "what its `undo` needs is kept without the secret in `TOKEN`, which it runs with"
        );
    }
}
