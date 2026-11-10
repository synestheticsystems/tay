//! Where things are in an environment's folder, `.tay/` in the stack's root.

use std::path::{Path, PathBuf};

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

    /// `state/intent`: whether the environment is meant to be up.
    pub fn intent(&self) -> PathBuf {
        self.state().join("intent")
    }

    /// `state/asked`: what the environment was last asked for.
    pub fn asked(&self) -> PathBuf {
        self.state().join("asked")
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

    /// The daemon's own log.
    pub fn daemon_log(&self) -> PathBuf {
        self.logs().join("tay").join("daemon.log")
    }
}
