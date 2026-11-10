//! The plan as `tay plan` prints it, in text or JSON.

use super::resolve::value::{Exact, Resolved};

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
        module: &'static str,
        way: Option<String>,
        /// Where the unit runs; `None` for a unit that is made, which runs
        /// nowhere.
        runs: Option<Running>,
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
pub struct Need {
    pub unit: String,
    pub because: Because,
}

/// Why a unit needs another.
#[derive(Clone, Debug)]
pub enum Because {
    /// Written in `needs`.
    Written,
    /// The unit is a command run inside the other's container (`in`).
    In,
    /// The unit uses this value, which the other produces by running.
    UsesProduced(String),
    /// The unit uses this value of the other, which is made.
    UsesMade(String),
}

#[derive(Debug)]
pub struct After {
    pub unit: String,
    pub comes_up: bool,
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

/// A container port in `publish`, and whether it is opened on the host.
#[derive(Debug)]
pub struct PublishedPort {
    pub container_port: i64,
    /// `None` when nothing opens it: it then uses nothing.
    pub opened: Option<OpenedPort>,
}

#[derive(Debug)]
pub struct OpenedPort {
    pub address: Shown,
    pub host_port: Shown,
    /// Opened with no use, by `always: true`.
    pub always: bool,
    /// What in the plan uses it from the host.
    pub by: Vec<Opener>,
}

/// A unit whose use of a published port from the host opens it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Opener {
    pub unit: String,
    pub how: OpenedBy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OpenedBy {
    /// The unit runs on the host and uses the address.
    UnitOnHost,
    /// tay makes the unit's readiness check, from the host.
    ReadinessCheck,
    /// A reference in the unit asks for the host's view, or for that of a
    /// unit on the host.
    View,
}

#[derive(Debug)]
pub struct RestartPolicy {
    pub on: Shown,
    pub attempts: Option<Shown>,
}

#[derive(Debug)]
pub struct GivenLine {
    pub name: String,
    pub endpoint: bool,
    /// The units whose running produces the value; empty when it is known before start.
    pub producers: Vec<String>,
    pub secret: bool,
    pub from_host: Seen,
    pub from_containers: Seen,
}

#[derive(Debug)]
pub enum Seen {
    Value(Shown),
    /// The value has no form for this asker, and why.
    Unreachable(String),
}

/// A value as printed: its text with holes shown, and its exact value when
/// all of it is known before start and none of it is secret.
#[derive(Clone, Debug)]
pub struct Shown {
    pub text: String,
    pub exact: Option<Exact>,
}

impl Shown {
    pub fn of(value: &Resolved) -> Shown {
        Shown {
            text: value.shown(),
            exact: value.exact(),
        }
    }

    pub fn word(text: &str) -> Shown {
        Shown {
            text: text.to_string(),
            exact: Some(Exact::Text(text.to_string())),
        }
    }

    pub fn number(n: i64) -> Shown {
        Shown {
            text: n.to_string(),
            exact: Some(Exact::Number(n)),
        }
    }
}
