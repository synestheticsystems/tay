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
    pub way: Way,
    pub needs: Vec<Need>,
    pub after: Vec<After>,
    pub ready: Readiness,
    pub restart: Option<RestartPolicy>,
    pub watch: Vec<Shown>,
    pub gives: Vec<GivenLine>,
}

#[derive(Debug)]
pub enum Way {
    /// A unit that only brings up what it needs.
    Group,
    Module {
        module: &'static str,
        alternative: Option<String>,
        runs: Running,
    },
}

#[derive(Debug)]
pub enum Running {
    OnHost,
    OwnContainer,
    InsideUnit(String),
    OneOffContainer,
    ByTay,
}

#[derive(Debug)]
pub struct Need {
    pub unit: String,
    pub because: Because,
}

#[derive(Debug)]
pub enum Because {
    /// Written in `needs`.
    Written,
    /// The unit uses this value, which the needed unit produces by running.
    Uses(String),
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
