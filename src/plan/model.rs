//! The plan as `tay plan` prints it, in text or JSON.

mod needs;
mod published;
mod values;

use crate::module::{Module, Settings};

pub use needs::{After, Because, Need};
pub use published::{OpenedBy, OpenedPort, Opener, PublishedPort};
pub use values::{GivenLine, Seen, Shown};

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
    pub mechanism: Mechanism,
    pub needs: Vec<Need>,
    pub after: Vec<After>,
    pub ready: Readiness,
    pub restart: Option<RestartPolicy>,
    pub watch: Vec<Shown>,
    pub publishes: Vec<PublishedPort>,
    pub gives: Vec<GivenLine>,
}

#[derive(Debug)]
pub enum Mechanism {
    /// A unit that only brings up what it needs.
    Group,
    Module {
        module: &'static dyn Module,
        way: Option<String>,
        /// Where the unit runs; `None` for a unit that is made, which runs
        /// nowhere.
        runs: Option<Running>,
        /// The module's own settings, every value resolved: what the unit
        /// is brought up from.
        #[cfg_attr(
            not(test),
            expect(
                dead_code,
                reason = "the engine, which is not built yet, brings each unit up from its settings"
            )
        )]
        settings: Box<dyn Settings>,
    },
}

#[derive(Debug)]
pub enum Running {
    OnHost,
    OwnContainer,
    InsideUnit(String),
    OneOffContainer,
}

#[derive(Debug)]
pub enum Readiness {
    Check {
        check: &'static str,
        target: Vec<Shown>,
        within: Shown,
    },
    /// No `ready` is written; how the unit's module knows it is ready.
    Default(&'static str),
    Group,
}

#[derive(Debug)]
pub struct RestartPolicy {
    pub on: Shown,
    pub attempts: Option<Shown>,
}
