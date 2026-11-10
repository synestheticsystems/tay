//! The engine: it brings a plan's units up through their modules'
//! operations, looks after the units that run for as long as they run,
//! says what is true of each unit now, and takes them down. It drives every
//! unit through the same few operations, and never asks which module a unit
//! uses.

mod bring_up;
mod check;
mod fill;
mod readiness;
mod settled;
mod state;
mod supervisor;
mod take_down;
mod units;

pub use bring_up::{Decision, bring_up};
pub use check::{check_now, find_again};
pub use state::{State, UnitState};
pub use supervisor::{Counts, Supervisor, Told};
pub use take_down::{Left, take_down};
