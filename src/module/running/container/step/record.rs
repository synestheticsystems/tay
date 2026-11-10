//! How the last step of a command run in a container ended, kept in the
//! unit's own folder by the daemon that saw it end. Docker keeps nothing of
//! such a step once it has ended: a program run inside another unit's
//! container leaves no trace there, and a container of a command's own is
//! removed once what it wrote is copied. So a daemon started afresh learns
//! how the step ended from this record, as it learns how a step on the host
//! ended from its keeper's.
//!
//! It is removed as a step begins, so that it never speaks for a step that
//! ended after it, and written only once the step's values are fetched, so
//! that a command it finds done gives what it wrote.

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{ContainerStepEnd, FoundStep};
use crate::disk::{DiskError, write_whole};
use crate::module::operations::{Failure, UnitPlace};

/// The record, in the unit's own folder.
const FILE: &str = "ended.json";

#[derive(Serialize, Deserialize)]
struct Record {
    attempt: u32,
    /// `check` or `run`.
    step: String,
    ended: ContainerStepEnd,
}

fn path(place: &UnitPlace) -> PathBuf {
    place.environment.layout.unit_state(&place.unit).join(FILE)
}

/// Keeps that step `step` of attempt `attempt` ended as `ended`.
pub fn keep(
    place: &UnitPlace,
    attempt: u32,
    step: &str,
    ended: &ContainerStepEnd,
) -> Result<(), Failure> {
    let path = path(place);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(DiskError::io(folder, "made"))?;
    }
    let record = Record {
        attempt,
        step: step.to_string(),
        ended: ended.clone(),
    };
    let json = serde_json::to_string_pretty(&record).expect("a step's record serializes");
    Ok(write_whole(&path, format!("{json}\n").as_bytes(), 0o600)?)
}

/// Forgets how the last step ended, as another begins.
pub fn forget(place: &UnitPlace) -> Result<(), Failure> {
    let path = path(place);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DiskError::io(path, "removed")(error).into()),
    }
}

/// The last step as its record says: ended, or never begun here when there
/// is no record.
pub fn found(place: &UnitPlace) -> Result<FoundStep, Failure> {
    let path = path(place);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(FoundStep::Never),
        Err(error) => return Err(DiskError::io(path, "read")(error).into()),
    };
    let record: Record =
        serde_json::from_str(&text).map_err(|source| DiskError::Unreadable { path, source })?;
    Ok(FoundStep::Ended {
        attempt: record.attempt,
        step: record.step,
        ended: record.ended,
    })
}
