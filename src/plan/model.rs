//! The plan: each unit that comes up, with its mechanism and that
//! mechanism's settings resolved, what it needs and why, how it is known to
//! be ready, and what it gives. Printing it is a view over this.

mod needs;
mod published;
mod values;

use crate::definition::Check;
use crate::module::{
    Duration, Module, OpenedPort, Operations, ReadyWithoutCheck, RestartOn, Settings, UnitKind,
};
use crate::value::{Filled, Placeholder, Unfilled, Value};

pub use needs::{After, Because, Need};
pub use published::{OpenedBy, Opener, Opening, PublishedPort};
pub use values::{Given, Seen, Unreachable};

#[derive(Debug)]
pub struct Plan {
    pub stack: String,
    pub root: String,
    pub profile: Option<String>,
    pub production: bool,
    /// The units `up` names: bringing the plan up has succeeded when each
    /// of them is ready.
    pub up: Vec<String>,
    /// Every unit that comes up, in an order it could start in.
    pub units: Vec<PlannedUnit>,
    /// Whether a value in the plan is `${host.listen}`.
    pub uses_host_listen: bool,
}

#[derive(Debug)]
pub struct PlannedUnit {
    pub name: String,
    pub doc: Option<String>,
    /// What the unit runs by; `None` for a group, which only brings up what
    /// it needs.
    pub mechanism: Option<PlannedMechanism>,
    pub needs: Vec<Need>,
    pub after: Vec<After>,
    pub gives: Vec<Given>,
}

/// The module a unit uses, under the way chosen for it, with its settings
/// and the fields that go with it, every value resolved.
#[derive(Debug)]
pub struct PlannedMechanism {
    pub module: &'static dyn Module,
    /// The way chosen, for a unit with ways.
    pub way: Option<String>,
    pub kind: UnitKind,
    /// The module's own settings: what the unit is brought up from.
    pub settings: Box<dyn Settings>,
    pub ready: Readiness,
    pub restart: Restart,
    pub watch: Vec<Value>,
    /// The ports a unit in a container of its own publishes, each opened or
    /// not.
    pub publishes: Vec<PublishedPort>,
}

impl PlannedMechanism {
    /// The unit's operations, from its settings and the ports the plan opens
    /// for it, every placeholder filled by `fill`; why a value could not be
    /// filled, when one cannot.
    pub fn operations(
        &self,
        fill: &mut dyn FnMut(&Placeholder) -> Result<Filled, Unfilled>,
    ) -> Result<Operations, Unfilled> {
        let mut opened = Vec::new();
        for published in &self.publishes {
            if let Some(opening) = &published.opening {
                opened.push(OpenedPort {
                    container_port: published.container_port,
                    address: opening.address.fill(fill)?,
                    host_port: opening.host_port.fill(fill)?,
                });
            }
        }
        self.settings.fill(opened, fill)
    }
}

#[derive(Debug)]
pub enum Readiness {
    /// The check `ready` writes, and the time it allows.
    Check {
        check: Check<Value>,
        within: Duration,
    },
    /// No `ready` is written: how the unit's module knows it is ready.
    WithoutCheck(ReadyWithoutCheck),
}

#[derive(Debug)]
pub struct Restart {
    pub on: RestartOn,
    /// The attempts in a row that may fail to reach ready; `None` for a
    /// unit that never restarts and writes none.
    pub attempts: Option<u64>,
}
