//! tay: Topology Automation via YAML.
//!
//! The crate reads a stack's definitions, plans it, and brings the plan up
//! through each environment's daemon. Only the command line is public,
//! because the `tay` binary is the only user of the rest; keeping the rest
//! private lets the compiler report what nothing uses.

pub mod cli;
mod clock;
mod control;
mod definition;
mod diagnostic;
mod disk;
mod engine;
mod keeper;
mod module;
mod plan;
mod print;
mod syntax;
mod value;
