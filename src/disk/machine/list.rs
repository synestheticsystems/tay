//! The machine's list of the environments tay knows: the folder of each and
//! the ports it keeps. The list is read freely, since it is only ever
//! replaced whole, and changed by one command at a time, under its lock. An
//! environment leaves it only when nothing of it is left: no folder, no
//! daemon, and nothing where its units run.

use std::io;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::Machine;
use crate::disk::{DiskError, Id, Lock, write_whole};

#[derive(Default, Serialize, Deserialize)]
struct ListFile {
    environments: Vec<Listed>,
}

/// One environment in the list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    pub id: Id,
    /// Its stack's root.
    pub folder: PathBuf,
    /// The port numbers it keeps.
    pub ports: Vec<u16>,
    /// Whether a module has made something of it outside the host, where
    /// its units run, which may be left there whatever its folder holds.
    #[serde(default)]
    pub made_outside: bool,
}

/// The list, held under its lock while it is changed.
pub struct List {
    environments: Vec<Listed>,
    path: PathBuf,
    _lock: Lock,
}

impl List {
    /// The list as it is, read without its lock.
    pub fn read(machine: &Machine) -> Result<Vec<Listed>, DiskError> {
        let path = machine.list();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(DiskError::io(path, "read")(error)),
        };
        let file: ListFile =
            serde_json::from_str(&text).map_err(|source| DiskError::Unreadable { path, source })?;
        Ok(file.environments)
    }

    /// The list, locked so that this command alone changes it until the
    /// list is dropped.
    pub fn lock(machine: &Machine) -> Result<List, DiskError> {
        let path = machine.list();
        let folder = machine.state.clone();
        std::fs::create_dir_all(&folder).map_err(DiskError::io(&folder, "made"))?;
        let lock = Lock::take(&machine.list_lock())?;
        let environments = List::read(machine)?;
        Ok(List {
            environments,
            path,
            _lock: lock,
        })
    }

    /// The environments of which nothing is left that this machine's
    /// folders can tell: no folder, and no daemon that answers. Whether
    /// anything of one is left where its units run is for the caller to
    /// find out, before it [forgets](List::forget) it.
    pub fn without_folder_or_daemon(machine: &Machine) -> Result<Vec<Listed>, DiskError> {
        Ok(List::read(machine)?
            .into_iter()
            .filter(|listed| !something_left(listed, machine))
            .collect())
    }

    /// Takes each environment of `gone`, of which nothing is left where its
    /// units run, off the list, unless its folder or its daemon has come
    /// back meanwhile, and unless another command is changing the list.
    pub fn forget(machine: &Machine, gone: &[Id]) -> Result<(), DiskError> {
        if gone.is_empty() {
            return Ok(());
        }
        let Some(lock) = Lock::try_take(&machine.list_lock())? else {
            return Ok(());
        };
        let mut list = List {
            environments: List::read(machine)?,
            path: machine.list(),
            _lock: lock,
        };
        let before = list.environments.len();
        list.environments
            .retain(|listed| !gone.contains(&listed.id) || something_left(listed, machine));
        match list.environments.len() == before {
            true => Ok(()),
            false => list.save(),
        }
    }

    pub fn environments(&self) -> &[Listed] {
        &self.environments
    }

    pub fn environments_mut(&mut self) -> &mut Vec<Listed> {
        &mut self.environments
    }

    /// Lists `ports` as the numbers the environment `id` keeps, and gives
    /// whether that changed the list. The environment must be in it:
    /// `tay up` puts it there before anything is made.
    pub fn set_ports(&mut self, id: &Id, mut ports: Vec<u16>) -> Result<bool, DiskError> {
        ports.sort_unstable();
        ports.dedup();
        let listed = self
            .environments
            .iter_mut()
            .find(|listed| listed.id == *id)
            .ok_or_else(|| DiskError::NotListed { id: id.to_string() })?;
        let changed = listed.ports != ports;
        listed.ports = ports;
        Ok(changed)
    }

    /// Notes that the environment `id` has made something outside the host;
    /// whether that changed the list. The environment must be in it.
    pub fn note_made_outside(&mut self, id: &Id) -> Result<bool, DiskError> {
        let listed = self
            .environments
            .iter_mut()
            .find(|listed| listed.id == *id)
            .ok_or_else(|| DiskError::NotListed { id: id.to_string() })?;
        let changed = !listed.made_outside;
        listed.made_outside = true;
        Ok(changed)
    }

    /// Writes the list as it now is.
    pub fn save(&self) -> Result<(), DiskError> {
        let file = ListFile {
            environments: self.environments.clone(),
        };
        let json = serde_json::to_string_pretty(&file).expect("the list always serializes");
        write_whole(&self.path, format!("{json}\n").as_bytes(), 0o644)
    }
}

/// Whether anything of an environment is left that this machine's folders
/// can tell: its folder, or its daemon, which answers on its socket until
/// it has stopped everything it started. A folder that is missing alone
/// does not take an environment off the list, since its daemon may still be
/// stopping what runs, and its containers, volumes or network may be left.
fn something_left(listed: &Listed, machine: &Machine) -> bool {
    listed.folder.exists() || UnixStream::connect(machine.socket(listed.id.as_str())).is_ok()
}
