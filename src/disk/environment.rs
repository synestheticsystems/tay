//! An environment of a stack on this machine: its folder, laid out under
//! `.tay/`, what identifies it, and whether it is meant to be up.

mod asked;
mod counts;
mod identity;
mod intent;
mod layout;

use std::path::PathBuf;

use super::Machine;

pub use asked::Asked;
pub use counts::{Count, Counts};
pub use identity::{Id, Identity};
pub use intent::Intent;
pub use layout::Layout;

/// An environment with an id of its own: its folder, its id, and the
/// machine it is on.
#[derive(Clone, Debug)]
pub struct Environment {
    pub layout: Layout,
    pub id: Id,
    pub machine: Machine,
}

impl Environment {
    /// `${environment.network}`: the name of the environment's one Docker
    /// network, which every container of it joins.
    pub fn network(&self) -> String {
        format!("tay-{}", self.id)
    }

    /// The socket its daemon listens on.
    pub fn socket(&self) -> PathBuf {
        self.machine.socket(self.id.as_str())
    }

    /// The lock a command holds while it starts the daemon.
    pub fn bootstrap_lock(&self) -> PathBuf {
        self.machine.bootstrap_lock(self.id.as_str())
    }
}
