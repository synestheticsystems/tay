//! Where things are in an environment's folder, `.tay/` in the stack's root.

use std::io;
use std::path::{Path, PathBuf};

use crate::disk::DiskError;

/// The folder inside a stack's root that holds its environment's own
/// choices and state.
const FOLDER: &str = ".tay";

/// The paths of one environment's `.tay/` folder. The `port`, `secret` and
/// `file` modules each keep their own record under `state/`, beside what
/// they write there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// The folder under `logs/` that holds the daemon's own log, which no
    /// unit may be named, so that no unit's log is put in it.
    pub const DAEMON_LOGS: &str = "tay";

    /// The layout of the environment whose stack's root is `root`.
    pub fn of(root: &Path) -> Layout {
        Layout {
            root: root.to_path_buf(),
        }
    }

    /// The stack's root, which the environment is.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `.tay/` itself.
    pub fn folder(&self) -> PathBuf {
        self.root.join(FOLDER)
    }

    /// `choices.yaml`: this environment's choices, which people write.
    pub fn choices(&self) -> PathBuf {
        self.folder().join("choices.yaml")
    }

    /// `id`: the environment's id.
    pub fn id(&self) -> PathBuf {
        self.folder().join("id")
    }

    /// `daemon.lock`: held for as long as the environment's daemon runs.
    pub fn daemon_lock(&self) -> PathBuf {
        self.folder().join("daemon.lock")
    }

    /// `state/`: what the environment keeps.
    pub fn state(&self) -> PathBuf {
        self.folder().join("state")
    }

    /// Makes `state/` if it is not there yet, as it is not until something
    /// of the environment is first kept.
    pub fn make_state(&self) -> Result<(), DiskError> {
        let state = self.state();
        std::fs::create_dir_all(&state).map_err(DiskError::io(&state, "made"))
    }

    /// `state/intent`: whether the environment is meant to be up.
    pub fn intent(&self) -> PathBuf {
        self.state().join("intent")
    }

    /// `state/asked`: what the environment was last asked for.
    pub fn asked(&self) -> PathBuf {
        self.state().join("asked")
    }

    /// `state/units/`: a folder for each unit that runs.
    pub fn units_state(&self) -> PathBuf {
        self.state().join("units")
    }

    /// `state/units/<unit>/`: what tay keeps of `unit`, a unit that runs:
    /// its lock, its record, the values it produced.
    pub fn unit_state(&self, unit: &str) -> PathBuf {
        self.units_state().join(unit)
    }

    /// `state/attempts.json`: what the daemon counts of each unit's attempts.
    pub fn attempts(&self) -> PathBuf {
        self.state().join("attempts.json")
    }

    /// `logs/`: a folder for each unit's log files, and `tay/`, the
    /// daemon's own.
    pub fn logs(&self) -> PathBuf {
        self.folder().join("logs")
    }

    /// `logs/<unit>/`: the folder of `unit`'s log files.
    pub fn unit_log(&self, unit: &str) -> PathBuf {
        self.logs().join(unit)
    }

    /// Removes everything in `.tay/` but `choices.yaml`, which people write,
    /// and `daemon.lock`, which whoever runs as the daemon holds.
    pub fn remove_kept(&self) -> Result<(), DiskError> {
        let folder = self.folder();
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(DiskError::io(folder, "read")(error)),
        };
        for entry in entries {
            let path = entry.map_err(DiskError::io(&folder, "read"))?.path();
            if path == self.choices() || path == self.daemon_lock() {
                continue;
            }
            let removed = match path.is_dir() {
                true => std::fs::remove_dir_all(&path),
                false => std::fs::remove_file(&path),
            };
            removed.map_err(DiskError::io(&path, "removed"))?;
        }
        Ok(())
    }

    /// The daemon's own log.
    pub fn daemon_log(&self) -> PathBuf {
        self.logs().join(Layout::DAEMON_LOGS).join("daemon.log")
    }
}
