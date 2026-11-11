//! The engine: it brings a plan's units up through their modules'
//! operations, looks after the units that run for as long as they run,
//! says what is true of each unit now, and takes them down. It drives every
//! unit through the same few operations, and never asks which module a unit
//! uses.

mod bring_up;
mod check;
mod fill;
mod leaving;
mod readiness;
mod settled;
mod started_from;
mod state;
mod supervisor;
mod take_down;
mod units;
mod unplanned;
mod watching;

pub use bring_up::{Decision, Outdated, Telling, bring_up};
pub use check::{check_now, find_again};
pub use leaving::{Leaving, undos_owed};
pub use state::{State, UnitState};
pub use supervisor::{Supervisor, Told, runs};
pub use take_down::{Left, take_down};
pub use unplanned::{check_unplanned, find_unplanned};
