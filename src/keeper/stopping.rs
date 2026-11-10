//! Stopping a unit's step: its child's whole group is asked to end, given
//! time, and what is left of it killed, and then the keeper is waited for.
//! What is stopped is the group the record names, or, when the record has
//! gone with the environment's folder, the one this daemon last saw run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::group::{GRACE, Group};
use super::process::RecordedProcess;
use super::record::Record;
use super::step::{LOOK, LaunchError, Look, StepFiles};

/// How long a keeper has to write its child once it is started, before a
/// step that is stopped is taken to have none.
const WRITES_CHILD: Duration = Duration::from_secs(2);

/// How long a keeper has to end once its child's group is gone.
const ENDS: Duration = Duration::from_secs(3);

impl StepFiles {
    /// Stops what of the unit runs: the step under way, by its child's
    /// group, and what is left of the group of a step whose keeper was lost,
    /// or whose record has gone with the environment's folder. Waits until
    /// the keeper has ended and nothing of the group is left. It blocks, for
    /// as long as the group takes to end.
    pub fn stop(&self) -> Result<(), LaunchError> {
        let child = match self.look()? {
            Look::Ended(..) => None,
            Look::Never => seen(self.folder()),
            Look::Lost(record) => record.child,
            Look::Running(record) => match record.child {
                Some(child) => Some(child),
                None => self.wait_for_child()?,
            },
        };
        if let Some(child) = child
            && !Group::led_by(child).stop().map_err(LaunchError::Signal)?
        {
            return Err(LaunchError::GroupStays { group: child.pid });
        }
        self.keeper_ended()?;
        forget(self.folder());
        Ok(())
    }

    /// The child of the step under way once its keeper has written it,
    /// waited for a while; `None` when the step ends meanwhile, or its keeper
    /// writes none.
    fn wait_for_child(&self) -> Result<Option<RecordedProcess>, LaunchError> {
        let until = Instant::now() + WRITES_CHILD;
        loop {
            match self.look()? {
                Look::Running(Record {
                    child: Some(child), ..
                })
                | Look::Lost(Record {
                    child: Some(child), ..
                }) => return Ok(Some(child)),
                Look::Running(_) if Instant::now() < until => std::thread::sleep(LOOK),
                _ => return Ok(None),
            }
        }
    }

    /// Waits until the unit's lock is free: its keeper has ended.
    fn keeper_ended(&self) -> Result<(), LaunchError> {
        let until = Instant::now() + ENDS + GRACE;
        while matches!(self.look()?, Look::Running(_)) {
            if Instant::now() >= until {
                return Err(LaunchError::KeeperStays);
            }
            std::thread::sleep(LOOK);
        }
        Ok(())
    }
}

/// The child of each unit's step under way, as this daemon last saw it in
/// the unit's record, by the unit's folder: what it stops when the
/// environment's folder, and the records in it, have gone while the stack
/// runs.
fn children() -> &'static Mutex<HashMap<PathBuf, RecordedProcess>> {
    static CHILDREN: OnceLock<Mutex<HashMap<PathBuf, RecordedProcess>>> = OnceLock::new();
    CHILDREN.get_or_init(Mutex::default)
}

pub(super) fn remember(folder: &Path, child: RecordedProcess) {
    children()
        .lock()
        .expect("the children seen are not poisoned")
        .insert(folder.to_path_buf(), child);
}

fn seen(folder: &Path) -> Option<RecordedProcess> {
    children()
        .lock()
        .expect("the children seen are not poisoned")
        .get(folder)
        .copied()
}

fn forget(folder: &Path) {
    children()
        .lock()
        .expect("the children seen are not poisoned")
        .remove(folder);
}
