//! The values a unit gives, as the plan holds them.

use crate::value::Value;

#[derive(Debug)]
pub struct Given {
    pub name: String,
    pub endpoint: bool,
    /// The units whose running produces the value; empty when it is known before start.
    pub producers: Vec<String>,
    pub secret: bool,
    pub from_host: Seen,
    pub from_containers: Seen,
}

/// A value given, as one asker sees it.
#[derive(Debug)]
pub enum Seen {
    Value(Value),
    /// The value has no form for this asker, and why.
    Unreachable(String),
}
