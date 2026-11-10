//! The modules whose units run something: a process on the host, a container,
//! and a command run once; the settings they share, each module still
//! deciding which of these it has; and what is found of a unit where it runs
//! whatever its settings.

pub mod command;
pub mod command_line;
pub mod container;
mod host;
mod mounts;
pub mod process;
mod variables;

use std::sync::Arc;
use std::time::Duration;

use super::description::Runs;
use super::operations::{
    Checking, Driving, Failure, Found, Outcome, Running, Steps, UnderWay, UnitPlace,
};
use crate::disk::Layout;

/// Whether anything of a unit that runs where `runs` says is running now,
/// from its place alone, whatever its settings: what `tay down` asks to
/// check that nothing is left, and a daemon asks of a unit whose settings it
/// cannot fill.
pub fn running_now(runs: &Runs, place: &UnitPlace) -> Result<bool, Failure> {
    match runs {
        Runs::OnHost => host::running_now(place),
        // tay runs nothing in a container yet.
        Runs::OwnContainer | Runs::InsideUnit(_) | Runs::OneOffContainer => Ok(false),
    }
}

/// The units of the environment `layout` lays out of which something runs
/// now, found where units run, whether or not the plan has them, and where
/// each runs: what `tay down` stops beside the units it knows, and what a
/// command asks before it says that nothing runs where no daemon does.
pub fn found_running(layout: &Layout) -> Result<Vec<(String, Runs)>, Failure> {
    // tay runs nothing in a container yet.
    Ok(host::found_running(layout)?
        .into_iter()
        .map(|unit| (unit, Runs::OnHost))
        .collect())
}

/// Stops what of a unit that runs where `runs` says is running, and
/// whatever it started in turn, from its place alone, whatever its settings.
/// It blocks until nothing of it is left.
pub fn stop(runs: &Runs, place: &UnitPlace) -> Result<(), Failure> {
    match runs {
        Runs::OnHost => host::stop(place),
        Runs::OwnContainer | Runs::InsideUnit(_) | Runs::OneOffContainer => Ok(()),
    }
}

/// The operations of a unit tay cannot run yet: each attempt fails, saying
/// so, and so does finding it again. Containers, and commands that run in
/// one, are the next part of the engine.
pub struct NotRunnable {
    /// What tay cannot run, as in "tay cannot run a `container` unit yet".
    pub what: &'static str,
}

#[derive(Debug, thiserror::Error)]
#[error("tay cannot run {0} yet")]
struct CannotRunYet(&'static str);

impl NotRunnable {
    fn reason(&self) -> String {
        CannotRunYet(self.what).to_string()
    }
}

impl Running for NotRunnable {
    fn start(self: Arc<Self>, _: UnitPlace, _: u32, _: bool, _: Steps) -> Driving {
        let reason = self.reason();
        Box::pin(async move { Outcome::Failed(reason) })
    }

    fn found(&self, _: &UnitPlace) -> Result<Found, Failure> {
        Err(CannotRunYet(self.what).into())
    }

    fn resume(self: Arc<Self>, _: UnitPlace, _: UnderWay, _: Steps) -> Driving {
        let reason = self.reason();
        Box::pin(async move { Outcome::Failed(reason) })
    }

    fn check(self: Arc<Self>, _: UnitPlace, _: Vec<String>, _: Duration) -> Checking {
        let reason = self.reason();
        Box::pin(async move { Err(reason) })
    }
}
