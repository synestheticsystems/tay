//! What each unit that runs was started from, kept in its folder of the
//! state folder whenever its module says it starts its own program: the
//! fingerprint of its settings its module takes, with each file the plan
//! says it uses, as the unit that writes or builds that file says it is
//! taken. A unit that runs when the environment is brought up again is
//! compared with what it would be started from now.

use std::path::Path;
use std::sync::Arc;

use super::supervisor::Supervisor;
use super::units::Units;
use crate::disk::{DiskError, write_whole};
use crate::module::{Fingerprint, Operations, Running, Taking, UnitKind, UnitPlace};
use crate::plan::{PlannedUnit, UsedFile};
use crate::value::{Filled, Placeholder, Unfilled};

/// The file in a unit's folder of the state folder that keeps what it was
/// started from.
const KEPT: &str = "started-from.json";

/// `settings`, what `planned` is started from as its module says, with
/// each file the plan says it uses, as each is now; why not, when a file's
/// path cannot be filled or the file cannot be taken.
pub fn with_files(
    supervisor: &Supervisor,
    planned: &PlannedUnit,
    settings: Fingerprint,
) -> Result<Fingerprint, String> {
    let place = supervisor.place(&planned.name);
    let mut taking = Taking::adding_to(&place, settings);
    let files = planned
        .mechanism
        .iter()
        .flat_map(|mechanism| &mechanism.files);
    let units = supervisor.snapshot();
    let given = units.given();
    let mut fill = |placeholder: &Placeholder| supervisor.fill(placeholder, &given);
    for file in files {
        let path = file.path.fill(&mut fill).map_err(|why| {
            format!(
                "the path of the file `{}` cannot be filled: {why}",
                file.name
            )
        })?;
        take(
            &units,
            file,
            Path::new(&path.reveal()),
            &mut fill,
            &mut taking,
        )?;
    }
    Ok(taking.taken())
}

/// Takes `file`, at `path`, into `taking`, as the unit that writes or
/// builds it says: one a unit that is made writes as that unit's module
/// says, and one a command builds by its content.
fn take(
    units: &Units,
    file: &UsedFile,
    path: &Path,
    fill: &mut dyn FnMut(&Placeholder) -> Result<Filled, Unfilled>,
    taking: &mut Taking,
) -> Result<(), String> {
    let writer = units
        .get(&file.of)
        .and_then(|known| known.planned.mechanism.as_ref())
        .filter(|mechanism| matches!(mechanism.kind, UnitKind::Made));
    let Some(writer) = writer else {
        return taking
            .file(&file.name, path)
            .map_err(|error| error.to_string());
    };
    match writer.operations(fill) {
        Ok(Operations::Made(made)) => made
            .take_written(taking, &file.name, path)
            .map_err(|failure| failure.to_string()),
        Ok(Operations::Running(_)) => unreachable!("a unit whose kind is made is made"),
        Err(why) => Err(format!(
            "what writes the file `{}` cannot be filled: {why}",
            file.name
        )),
    }
}

/// What changed in what `planned`, which runs, would be started from now
/// with `running`, from what it was started from, as a person reads it:
/// `None` when nothing did; why it cannot be told, when it cannot.
pub async fn changed(
    supervisor: &Supervisor,
    planned: &PlannedUnit,
    running: Arc<dyn Running>,
) -> Result<Option<String>, String> {
    let place = supervisor.place(&planned.name);
    let settings = running
        .started_from(place.clone())
        .await
        .map_err(|failure| failure.to_string())?;
    let now = with_files(supervisor, planned, settings)?;
    match kept(&place).map_err(|error| error.to_string())? {
        Some(kept) => {
            let changed = kept.changed(&now);
            Ok((!changed.is_empty()).then(|| format!("{changed} changed")))
        }
        // What it was started from is kept as it starts, so a unit that
        // runs with none kept started before that could be: what it runs
        // from is not known.
        None => Ok(Some("what it was started from is not known".to_string())),
    }
}

/// Keeps `fingerprint` as what the unit at `place` was started from.
pub fn keep(place: &UnitPlace, fingerprint: &Fingerprint) -> Result<(), DiskError> {
    let folder = place.environment.layout.unit_state(&place.unit);
    std::fs::create_dir_all(&folder).map_err(DiskError::io(&folder, "made"))?;
    let json = serde_json::to_string_pretty(fingerprint).expect("a fingerprint serializes");
    write_whole(&folder.join(KEPT), format!("{json}\n").as_bytes(), 0o600)
}

/// What the unit at `place` was started from, as it was kept; `None` when
/// nothing is kept.
pub fn kept(place: &UnitPlace) -> Result<Option<Fingerprint>, DiskError> {
    let path = place.environment.layout.unit_state(&place.unit).join(KEPT);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DiskError::io(path, "read")(error)),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|source| DiskError::Unreadable { path, source })
}
