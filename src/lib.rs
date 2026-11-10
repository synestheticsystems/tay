//! tay: Topology Automation via YAML.
//!
//! The crate reads a stack's definitions and plans it. Only the command line
//! is public, because the `tay` binary is the only user of the rest so far;
//! keeping the rest private lets the compiler report what nothing uses.

pub mod cli;
mod definition;
mod diagnostic;
mod module;
mod plan;
mod syntax;
mod value;
