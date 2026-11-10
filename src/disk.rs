//! What tay keeps on disk: each environment's `.tay/` folder in the stack's
//! root, and per machine, the list of environments tay knows with the ports
//! each keeps, the folder a stack's environments share, and the short
//! folders their daemons' sockets are in. What a module keeps in an
//! environment's state folder is the module's own: the ports, the secrets
//! and the files.

mod environment;
mod lock;
mod machine;
mod whole;

use std::io;
use std::path::PathBuf;

pub use environment::{Asked, Environment, Id, Identity, Intent, Layout};
pub use lock::Lock;
pub use machine::{List, Listed, Machine, shared_folder, shared_folders, socket_fits};
pub use whole::write_whole;

/// What went wrong with something tay keeps on disk.
#[derive(Debug, thiserror::Error)]
pub enum DiskError {
    #[error("`{}` cannot be {doing}: {source}", path.display())]
    Io {
        path: PathBuf,
        /// What was being done, as in "`path` cannot be read".
        doing: &'static str,
        source: io::Error,
    },
    #[error("`{}` is not what tay writes there: {source}", path.display())]
    Unreadable {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("`{}` holds `{found}`, which is not {wanted}", path.display())]
    Unexpected {
        path: PathBuf,
        found: String,
        wanted: &'static str,
    },
    #[error(
        "`{}` stayed locked for {} seconds: another tay holds it", path.display(), lock::WAIT.as_secs()
    )]
    StayedLocked { path: PathBuf },
    #[error("the folder `{}` is not this account's alone, so tay does not use it", path.display())]
    NotOwn { path: PathBuf },
    #[error("the home folder is not known: HOME is not set, so neither is {what}")]
    NoHome { what: &'static str },
    #[error(
        "the daemon's socket would be `{}`, longer than the system lets a socket's path be: set XDG_RUNTIME_DIR to a shorter folder",
        path.display()
    )]
    SocketTooLong { path: PathBuf },
    #[error(
        "environment {id} is not in this machine's list of environments; `tay up` puts it there"
    )]
    NotListed { id: String },
}

impl DiskError {
    /// `io` failed while `path` was being `doing`.
    pub fn io(
        path: impl Into<PathBuf>,
        doing: &'static str,
    ) -> impl FnOnce(io::Error) -> DiskError {
        let path = path.into();
        move |source| DiskError::Io {
            path,
            doing,
            source,
        }
    }
}
