//! The engine: it brings a plan's units up through their modules'
//! operations, says what is true of each unit now, and takes them down. It
//! drives every unit through the same few operations, and never asks which
//! module a unit uses.

mod bring_up;
mod check;
mod fill;
mod settled;
mod state;
mod take_down;
mod units;

pub use bring_up::{Decision, Progress, bring_up};
pub use check::{check_now, find_again};
pub use state::{State, UnitState};
pub use take_down::{Left, take_down};
pub use units::Units;
