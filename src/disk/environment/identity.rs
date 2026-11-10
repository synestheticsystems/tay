//! What identifies an environment: its id, random, made the first time the
//! environment is brought up and kept in `.tay/id`. Moving the folder keeps
//! it. A copied folder is noticed, because the machine's list has its id at
//! another folder that still holds that id, and is given an id of its own.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

use super::{Environment, Layout};
use crate::disk::machine::List;
use crate::disk::{DiskError, Listed, Machine, write_whole};

/// How many random bytes an id is made from; it is written as twice as
/// many hexadecimal digits.
const ID_BYTES: usize = 6;

/// An environment's id, short enough for a socket's path and a Docker
/// network's name, and unique among the environments on this machine.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(String);

/// Which environment a folder is.
#[derive(Debug)]
pub enum Identity {
    /// It has never been brought up, so it has no id.
    New,
    /// It has an id of its own.
    Own(Environment),
    /// It is a copy of the environment at `original`, whose id it holds.
    /// It is given one of its own when it is next brought up.
    CopyOf { original: PathBuf },
}

impl Id {
    fn random() -> Id {
        let mut bytes = [0u8; ID_BYTES];
        OsRng.fill_bytes(&mut bytes);
        Id(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    /// The id in `layout`'s folder; `None` when it has none.
    fn read(layout: &Layout) -> Result<Option<Id>, DiskError> {
        let path = layout.id();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(DiskError::io(path, "read")(error)),
        };
        let id = text.trim();
        let hex = id.len() == 2 * ID_BYTES
            && id
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
        if !hex {
            return Err(DiskError::Unexpected {
                path,
                found: id.to_string(),
                wanted: "an environment's id",
            });
        }
        Ok(Some(Id(id.to_string())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Environment {
    /// Which environment the folder `layout` lays out is, as things are:
    /// nothing is changed.
    pub fn identify(layout: &Layout, machine: &Machine) -> Result<Identity, DiskError> {
        let Some(id) = Id::read(layout)? else {
            return Ok(Identity::New);
        };
        Ok(match original(&List::read(machine)?, &id, layout.root()) {
            Some(original) => Identity::CopyOf { original },
            None => Identity::Own(Environment {
                layout: layout.clone(),
                id,
                machine: machine.clone(),
            }),
        })
    }

    /// The environment at `layout`, given an id if it has none and a new
    /// one if it is a copy, and entered in the machine's list at its
    /// folder: at its new folder when it has moved.
    pub fn establish(layout: &Layout, machine: &Machine) -> Result<Environment, DiskError> {
        let root = layout.root();
        // An environment already listed at its folder, alone there, is
        // established, and the list is not locked to find that out.
        if let Some(id) = Id::read(layout)? {
            let listed = List::read(machine)?;
            let alone = listed
                .iter()
                .filter(|listed| listed.folder == root)
                .all(|listed| listed.id == id);
            if alone
                && listed
                    .iter()
                    .any(|listed| listed.id == id && listed.folder == root)
            {
                return Ok(Environment {
                    layout: layout.clone(),
                    id,
                    machine: machine.clone(),
                });
            }
        }
        let mut list = List::lock(machine)?;
        let id = match Id::read(layout)? {
            Some(id) if original(list.environments(), &id, root).is_none() => id,
            _ => {
                let id = loop {
                    let id = Id::random();
                    if !list.environments().iter().any(|listed| listed.id == id) {
                        break id;
                    }
                };
                let folder = layout.folder();
                std::fs::create_dir_all(&folder).map_err(DiskError::io(&folder, "made"))?;
                write_whole(&layout.id(), format!("{id}\n").as_bytes(), 0o644)?;
                id
            }
        };
        let environment = Environment {
            layout: layout.clone(),
            id,
            machine: machine.clone(),
        };
        let environments = list.environments_mut();
        // A folder is one environment: one listed at it under another id
        // is gone.
        environments.retain(|listed| listed.folder != root || listed.id == environment.id);
        match environments
            .iter_mut()
            .find(|listed| listed.id == environment.id)
        {
            Some(listed) => listed.folder = root.to_path_buf(),
            // The ports it keeps are listed when they are next chosen.
            None => environments.push(Listed {
                id: environment.id.clone(),
                folder: root.to_path_buf(),
                ports: Vec::new(),
                made_outside: false,
            }),
        }
        list.save()?;
        Ok(environment)
    }
}

/// The folder whose environment `id` is, when it is another than `root`
/// and still holds that id: `root` is then a copy of it.
fn original(listed: &[Listed], id: &Id, root: &Path) -> Option<PathBuf> {
    listed
        .iter()
        .filter(|listed| listed.id == *id && listed.folder != root)
        .find(
            |listed| matches!(Id::read(&Layout::of(&listed.folder)), Ok(Some(held)) if held == *id),
        )
        .map(|listed| listed.folder.clone())
}
