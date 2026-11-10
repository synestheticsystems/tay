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
    /// The value has no form for this asker, for each of these reasons.
    Unreachable(Vec<Unreachable>),
}

/// Why the host cannot have an address in a value a unit gives.
#[derive(Debug)]
pub enum Unreachable {
    /// The container publishes no host port for this port of its.
    NotPublished { port: String },
    /// The container publishes this port of its, and nothing opens it.
    NotOpened { port: i64 },
}
