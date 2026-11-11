//! An environment of a stack on this machine: its folder, laid out under
//! `.tay/`, what identifies it, and whether it is meant to be up.

mod asked;
mod counts;
pub mod daemon_socket;
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

    /// The socket a daemon of it started in this session listens on, in
    /// the short folder named by its id in this session's runtime folder.
    /// A daemon started in another session, as one a service manager
    /// starts, listens elsewhere: a command finds the daemon that runs by
    /// the path it wrote.
    pub fn socket(&self) -> PathBuf {
        self.machine.socket(self.id.as_str())
    }
}
