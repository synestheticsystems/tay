//! What the daemon counts of each unit's attempts, kept on disk in
//! `state/attempts.json`, so that a daemon started afresh counts on: the
//! attempt last begun, how many attempts in a row before it did not reach
//! ready, and whether it has. A unit found ended is restarted only while its
//! `attempts` are not used up, and one found under way that was ready is
//! judged as a ready unit is.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::disk::{DiskError, Layout, write_whole};

/// One unit's count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Count {
    pub attempt: u32,
    /// Attempts in a row before this one that did not reach ready.
    pub in_a_row: u64,
    /// Whether this attempt has reached ready.
    pub ready: bool,
}

/// Every unit's count, as kept.
pub struct Counts {
    path: PathBuf,
    units: Mutex<BTreeMap<String, Count>>,
}

impl Counts {
    /// The counts kept for the environment `layout` lays out; none when
    /// none are.
    pub fn read(layout: &Layout) -> Result<Counts, DiskError> {
        let path = layout.attempts();
        let units = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|source| DiskError::Unreadable {
                path: path.clone(),
                source,
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(DiskError::io(path, "read")(error)),
        };
        Ok(Counts {
            path,
            units: Mutex::new(units),
        })
    }

    /// No counts, for an environment whose kept counts cannot be read.
    pub fn none(layout: &Layout) -> Counts {
        Counts {
            path: layout.attempts(),
            units: Mutex::default(),
        }
    }

    /// `unit`'s count, when it is of `attempt`.
    pub fn of(&self, unit: &str, attempt: u32) -> Option<Count> {
        self.units
            .lock()
            .expect("the counts are not poisoned")
            .get(unit)
            .copied()
            .filter(|count| count.attempt == attempt)
    }

    /// Keeps `count` as `unit`'s.
    pub fn keep(&self, unit: &str, count: Count) -> Result<(), DiskError> {
        let mut units = self.units.lock().expect("the counts are not poisoned");
        units.insert(unit.to_string(), count);
        let json = serde_json::to_string_pretty(&*units).expect("counts serialize");
        write_whole(&self.path, format!("{json}\n").as_bytes(), 0o644)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_is_kept_and_read_back_for_its_attempt_only() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".tay/state")).unwrap();
        let layout = Layout::of(root.path());
        let count = Count {
            attempt: 3,
            in_a_row: 2,
            ready: false,
        };
        Counts::read(&layout).unwrap().keep("web", count).unwrap();
        let read = Counts::read(&layout).unwrap();
        assert_eq!(read.of("web", 3), Some(count));
        assert_eq!(read.of("web", 4), None);
        assert_eq!(read.of("api", 3), None);
    }
}
