//! The plan: each unit that comes up, with its mechanism and that
//! mechanism's settings resolved, what it needs and why, how it is known to
//! be ready, and what it gives. Printing it is a view over this.

mod needs;
mod published;
mod values;

use crate::definition::Check;
use crate::module::{
    Duration, FromPlan, Module, OpenedPort, Operations, ReadyWithoutCheck, RestartOn, Settings,
    UnitKind,
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
    /// Whether a value in the plan is `${host.listen}` where it is learnt
    /// as the environment is brought up.
    pub uses_host_listen: bool,
}

impl Plan {
    /// Whether a unit of the plan runs in a container, as its module says.
    pub fn runs_in_a_container(&self) -> bool {
        self.units
            .iter()
            .filter_map(|unit| unit.mechanism.as_ref())
            .any(|mechanism| match &mechanism.kind {
                UnitKind::Runs(runs) => runs.in_a_container(),
                UnitKind::Made => false,
            })
    }

    /// Each unit of this plan, with a published port of it, that this plan
    /// opens and `other` does not: what runs of the unit as `other` gives
    /// it would have to be replaced to run as this plan does.
    pub fn opening_more_than(&self, other: &Plan) -> Vec<(&str, i64)> {
        let opened = |plan: &Plan, unit: &str, port: i64| {
            plan.units
                .iter()
                .filter(|planned| planned.name == unit)
                .filter_map(|planned| planned.mechanism.as_ref())
                .flat_map(|mechanism| &mechanism.publishes)
                .any(|published| published.container_port == port && published.opening.is_some())
        };
        let mut more = Vec::new();
        for planned in &self.units {
            let Some(mechanism) = &planned.mechanism else {
                continue;
            };
            for published in &mechanism.publishes {
                let port = published.container_port;
                if published.opening.is_some() && !opened(other, &planned.name, port) {
                    more.push((planned.name.as_str(), port));
                }
            }
        }
        more
    }
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
    /// The units whose volumes it mounts.
    pub mounts_volumes_of: Vec<String>,
    /// The files tay knows of that its settings use: what it is started
    /// from beside them.
    pub files: Vec<UsedFile>,
    /// The units that run that it needs, a group standing for those it
    /// needs in turn, in the order units start in. What a unit that is made
    /// gives it is in its values already.
    pub needs_that_run: Vec<String>,
}

/// A file a unit uses that tay knows of: one a unit that is made writes,
/// named by that unit, or one a command builds, named
/// `unit.outputs.name`; the unit that writes or builds it, and its path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsedFile {
    pub name: String,
    pub of: String,
    pub path: Value,
}

impl PlannedMechanism {
    /// The unit's operations, from its settings, the ports the plan opens
    /// for it and the units that run that it needs, every placeholder
    /// filled by `fill`; why a value could not be filled, when one cannot.
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
        let from_plan = FromPlan {
            opened,
            needs_that_run: self.needs_that_run.clone(),
        };
        self.settings.fill(from_plan, fill)
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
