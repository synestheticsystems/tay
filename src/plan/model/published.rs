//! A container's published ports, as the plan says it: opened or not, and
//! what opens each.

use super::Shown;

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
