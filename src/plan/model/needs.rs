//! What a unit needs and waits for, as the plan says it.

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
