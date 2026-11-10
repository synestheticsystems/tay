//! The machine's list of the environments tay knows: the folder of each and
//! the ports it keeps. The list is read freely, since it is only ever
//! replaced whole, and changed by one command at a time, under its lock.

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
    /// list is dropped. An environment of which nothing is left leaves it.
    pub fn lock(machine: &Machine) -> Result<List, DiskError> {
        let path = machine.list();
        let folder = machine.state.clone();
        std::fs::create_dir_all(&folder).map_err(DiskError::io(&folder, "made"))?;
        let lock = Lock::take(&machine.list_lock())?;
        let mut environments = List::read(machine)?;
        environments.retain(|listed| something_left(listed, machine));
        Ok(List {
            environments,
            path,
            _lock: lock,
        })
    }

    /// The list, with every environment of which nothing is left taken
    /// off it, when no other command is changing it; as it is otherwise.
    pub fn known(machine: &Machine) -> Result<Vec<Listed>, DiskError> {
        let folder = &machine.state;
        std::fs::create_dir_all(folder).map_err(DiskError::io(folder, "made"))?;
        let Some(lock) = Lock::try_take(&machine.list_lock())? else {
            return List::read(machine);
        };
        let mut environments = List::read(machine)?;
        let listed = environments.len();
        environments.retain(|listed| something_left(listed, machine));
        if environments.len() != listed {
            List {
                environments: environments.clone(),
                path: machine.list(),
                _lock: lock,
            }
            .save()?;
        }
        Ok(environments)
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

    /// Writes the list as it now is.
    pub fn save(&self) -> Result<(), DiskError> {
        let file = ListFile {
            environments: self.environments.clone(),
        };
        let json = serde_json::to_string_pretty(&file).expect("the list always serializes");
        write_whole(&self.path, format!("{json}\n").as_bytes(), 0o644)
    }
}

/// Whether anything of an environment is left: its folder, or its daemon,
/// which answers on its socket until it has stopped everything it started.
/// A folder that is missing alone does not take an environment off the
/// list, since its daemon may still be stopping what runs.
fn something_left(listed: &Listed, machine: &Machine) -> bool {
    listed.folder.exists() || UnixStream::connect(machine.socket(listed.id.as_str())).is_ok()
}
