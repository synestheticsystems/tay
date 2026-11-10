//! The plan: each unit that comes up, with its mechanism and that
//! mechanism's settings resolved, what it needs and why, how it is known to
//! be ready, and what it gives. Printing it is a view over this.

mod needs;
mod published;
mod values;

use crate::definition::Check;
use crate::module::{Duration, Module, ReadyWithoutCheck, RestartOn, Settings, UnitKind};
use crate::value::Value;

pub use needs::{After, Because, Need};
pub use published::{OpenedBy, OpenedPort, Opener, PublishedPort};
pub use values::{Given, Seen};

#[derive(Debug)]
pub struct Plan {
    pub stack: String,
    pub root: String,
    pub profile: Option<String>,
    pub production: bool,
    /// Every unit that comes up, in an order it could start in.
    pub units: Vec<PlannedUnit>,
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the engine, which is not built yet, brings each unit up from its settings"
        )
    )]
    pub settings: Box<dyn Settings>,
    pub ready: Readiness,
    pub restart: Restart,
    pub watch: Vec<Value>,
    /// The ports a unit in a container of its own publishes, each opened or
    /// not.
    pub publishes: Vec<PublishedPort>,
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
