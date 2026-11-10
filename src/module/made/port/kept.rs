//! The ports an environment keeps. Its own record, `state/ports.json`, says
//! which unit keeps which number; the machine's list of environments says
//! which numbers each environment keeps. Numbers are chosen under the
//! list's lock, so two environments starting at once are never given one.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::disk::{DiskError, Environment, Id, List, write_whole};

/// The environment's record, in its state folder.
const PORTS: &str = "ports.json";

/// The record as it is written: whose it is, and the number each unit
/// keeps. A copied folder holds its original's record, which names the
/// original and so is not the copy's.
#[derive(Serialize, Deserialize)]
struct Record {
    environment: Id,
    units: BTreeMap<String, u16>,
}

/// The ports of every environment on this machine, held under the list's
/// lock while one environment chooses.
pub struct Ports<'e> {
    environment: &'e Environment,
    list: List,
    /// This environment's, by unit.
    kept: BTreeMap<String, u16>,
}

/// What keeps a port number, other than the unit asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holder {
    /// Another environment on this machine, at its folder.
    Environment { folder: PathBuf },
    /// Another unit of this environment.
    Unit { name: String },
}

impl<'e> Ports<'e> {
    /// The ports, locked until they are dropped. The machine's list is
    /// brought up to date with every number this environment keeps, which
    /// it may have lost, as it does when the environment's folder is moved
    /// while it is down.
    pub fn lock(environment: &'e Environment) -> Result<Ports<'e>, DiskError> {
        let mut list = List::lock(&environment.machine)?;
        let kept = Ports::kept_by(environment)?;
        if list.set_ports(&environment.id, kept.values().copied().collect())? {
            list.save()?;
        }
        Ok(Ports {
            environment,
            list,
            kept,
        })
    }

    /// The numbers `environment` keeps, by unit, read without the lock.
    pub fn kept_by(environment: &Environment) -> Result<BTreeMap<String, u16>, DiskError> {
        let path = record(environment);
        let record: Record = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|source| DiskError::Unreadable { path, source })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(error) => return Err(DiskError::io(path, "read")(error)),
        };
        Ok(match record.environment == environment.id {
            true => record.units,
            false => BTreeMap::new(),
        })
    }

    /// The number `unit` keeps.
    pub fn kept(&self, unit: &str) -> Option<u16> {
        self.kept.get(unit).copied()
    }

    /// What keeps `number`, other than `unit`: another environment, or
    /// another unit of this one.
    pub fn holder(&self, number: u16, unit: &str) -> Option<Holder> {
        if let Some((name, _)) = self
            .kept
            .iter()
            .find(|(name, kept)| *name != unit && **kept == number)
        {
            return Some(Holder::Unit { name: name.clone() });
        }
        self.list
            .environments()
            .iter()
            .find(|listed| listed.id != self.environment.id && listed.ports.contains(&number))
            .map(|listed| Holder::Environment {
                folder: listed.folder.clone(),
            })
    }

    /// Records that `unit` keeps `number`, in this environment's record and
    /// in the machine's list.
    pub fn keep(&mut self, unit: &str, number: u16) -> Result<(), DiskError> {
        self.kept.insert(unit.to_string(), number);
        let state = self.environment.layout.state();
        std::fs::create_dir_all(&state).map_err(DiskError::io(&state, "made"))?;
        let written = Record {
            environment: self.environment.id.clone(),
            units: self.kept.clone(),
        };
        let json = serde_json::to_string_pretty(&written).expect("port numbers serialize");
        write_whole(
            &record(self.environment),
            format!("{json}\n").as_bytes(),
            0o644,
        )?;
        let ports = self.kept.values().copied().collect();
        self.list.set_ports(&self.environment.id, ports)?;
        self.list.save()
    }
}

fn record(environment: &Environment) -> PathBuf {
    environment.layout.state().join(PORTS)
}
