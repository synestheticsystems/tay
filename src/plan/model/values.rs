//! Values as the plan prints them.

use crate::plan::resolve::value::{Exact, Resolved};

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
