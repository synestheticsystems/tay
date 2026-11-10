//! This machine's folders for tay, outside any environment: the list of
//! environments, the folders a stack's environments share, and the short
//! folders their daemons' sockets are in. `XDG_STATE_HOME`, `XDG_DATA_HOME`
//! and `XDG_RUNTIME_DIR` move them where they are set.

mod list;

use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::DiskError;

pub use list::{List, Listed};

#[derive(Clone, Debug)]
pub struct Machine {
    /// The list of environments tay knows.
    state: PathBuf,
    /// A folder for each stack, shared by one person's environments of it.
    shared: PathBuf,
    /// A short folder for each environment, holding its daemon's socket.
    runtime: PathBuf,
}

impl Machine {
    /// The folders, from the environment variables tay was started with:
    /// `$XDG_STATE_HOME/tay` or `~/.local/state/tay`, `$XDG_DATA_HOME/tay`
    /// or `~/.local/share/tay`, and `$XDG_RUNTIME_DIR/tay` or `tay-<uid>`
    /// in the temporary folder.
    pub fn from_env() -> Result<Machine, DiskError> {
        let home = home();
        let state = xdg("XDG_STATE_HOME")
            .or_else(|| home.as_ref().map(|home| home.join(".local/state")))
            .ok_or(DiskError::NoHome {
                what: "the folder for the list of environments",
            })?;
        let shared = shared_folders().ok_or(DiskError::NoHome {
            what: "the folder environments share",
        })?;
        let runtime = match xdg("XDG_RUNTIME_DIR") {
            Some(runtime) => runtime.join("tay"),
            None => {
                std::env::temp_dir().join(format!("tay-{}", rustix::process::getuid().as_raw()))
            }
        };
        Ok(Machine {
            state: state.join("tay"),
            shared,
            runtime,
        })
    }

    /// The list of environments tay knows.
    pub fn list(&self) -> PathBuf {
        self.state.join("environments.json")
    }

    /// The lock that one command at a time holds to change the list.
    fn list_lock(&self) -> PathBuf {
        self.state.join("environments.lock")
    }

    /// The folder `stack`'s environments share.
    pub fn shared(&self, stack: &str) -> PathBuf {
        shared_folder(&self.shared, stack)
    }

    /// Makes the short folder named by the environment's id, which holds
    /// its daemon's socket and the lock a command takes to start the
    /// daemon, for this account alone.
    pub fn make_socket_folder(&self, id: &str) -> Result<(), DiskError> {
        let socket = self.socket(id);
        if !socket_fits(&socket) {
            return Err(DiskError::SocketTooLong { path: socket });
        }
        own_folder(&self.runtime)?;
        own_folder(&self.runtime.join(id))
    }

    /// The socket of the daemon of the environment whose id is `id`.
    pub fn socket(&self, id: &str) -> PathBuf {
        self.runtime.join(id).join("daemon.sock")
    }

    /// The lock a command holds while it starts the daemon of the
    /// environment whose id is `id`, so that one daemon is started.
    pub fn bootstrap_lock(&self, id: &str) -> PathBuf {
        self.runtime.join(id).join("bootstrap.lock")
    }
}

/// The longest path a socket may have, in bytes: the system's limit, less
/// the byte that ends it.
#[cfg(target_os = "linux")]
const SOCKET_PATH: usize = 107;
#[cfg(not(target_os = "linux"))]
const SOCKET_PATH: usize = 103;

/// Whether a socket can be had at `path`, whose length the system limits.
pub fn socket_fits(path: &Path) -> bool {
    path.as_os_str().len() <= SOCKET_PATH
}

/// The folder that holds each stack's shared folder, `$XDG_DATA_HOME/tay`
/// or `~/.local/share/tay`; `None` when neither is known.
pub fn shared_folders() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME")
        .or_else(|| home().map(|home| home.join(".local/share")))
        .map(|data| data.join("tay"))
}

/// The folder `stack`'s environments share, in `folders`, which holds one
/// for each stack.
pub fn shared_folder(folders: &Path, stack: &str) -> PathBuf {
    folders.join(stack)
}

/// `HOME`, when it is an absolute path.
fn home() -> Option<PathBuf> {
    absolute(std::env::var_os("HOME")?.into())
}

/// An XDG base folder from its variable. The XDG specification says a
/// relative path in one is to be ignored.
fn xdg(variable: &str) -> Option<PathBuf> {
    absolute(std::env::var_os(variable)?.into())
}

fn absolute(path: PathBuf) -> Option<PathBuf> {
    path.is_absolute().then_some(path)
}

/// `folder`, made for this account alone if it is missing. One that is
/// there already must be this account's and closed to others, since a
/// socket in it decides who may control an environment.
fn own_folder(folder: &Path) -> Result<(), DiskError> {
    match std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
    {
        Ok(()) => {}
        Err(error) => return Err(DiskError::io(folder, "made")(error)),
    }
    let metadata = std::fs::metadata(folder).map_err(DiskError::io(folder, "read"))?;
    let own = metadata.uid() == rustix::process::getuid().as_raw();
    if !own || metadata.permissions().mode() & 0o077 != 0 {
        return Err(DiskError::NotOwn {
            path: folder.to_path_buf(),
        });
    }
    Ok(())
}
