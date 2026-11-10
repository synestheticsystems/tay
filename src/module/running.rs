//! The modules whose units run something: a process on the host, a container,
//! and a command run once; and the settings they share, each module still
//! deciding which of these it has.

pub mod command;
pub mod command_line;
pub mod container;
mod mounts;
pub mod process;
mod variables;

use super::operations::{Attempt, Ending, Failure, Running, UnitPlace};

/// The operations of a unit whose module tay cannot run yet: each fails,
/// saying so. The keeper, which runs host processes and commands, and the
/// containers are the next parts of the engine.
pub struct NotRunnable {
    /// The module's name, as `uses` writes it.
    pub module: &'static str,
}

#[derive(Debug, thiserror::Error)]
#[error("tay cannot run a `{0}` unit yet")]
struct CannotRunYet(&'static str);

impl Running for NotRunnable {
    fn start(&self, _: &UnitPlace<'_>) -> Result<Attempt, Failure> {
        Err(CannotRunYet(self.module).into())
    }

    fn find(&self, _: &UnitPlace<'_>) -> Result<Attempt, Failure> {
        Err(CannotRunYet(self.module).into())
    }

    fn ended(&self, _: &UnitPlace<'_>) -> Result<Option<Ending>, Failure> {
        Err(CannotRunYet(self.module).into())
    }

    fn stop(&self, _: &UnitPlace<'_>) -> Result<(), Failure> {
        Err(CannotRunYet(self.module).into())
    }
}
