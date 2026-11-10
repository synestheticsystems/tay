//! A container's published ports, as the plan says it: opened on the host or
//! not, and what opens each.

use crate::value::Value;

/// A container port in `publish`, and whether it is opened on the host.
#[derive(Debug)]
pub struct PublishedPort {
    pub container_port: i64,
    /// `None` when nothing opens it: it then uses nothing.
    pub opening: Option<Opening>,
}

/// Where a published port is opened on the host, and what opens it.
#[derive(Debug)]
pub struct Opening {
    pub address: Value,
    pub host_port: Value,
    /// Opened with no use, by `always: true`.
    pub always: bool,
    /// What in the plan uses it from the host.
    pub openers: Vec<Opener>,
}

/// A unit whose use of a published port from the host opens it, and how.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Opener {
    pub unit: String,
    pub how: OpenedBy,
}

/// How a unit's use opens a published port.
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
