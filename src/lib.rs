//! tay: Topology Automation via YAML.
//!
//! The library reads a stack's definitions and plans it; the `tay` binary is
//! its command line.

pub mod cli;
pub mod definition;
pub mod diagnostic;
pub mod expr;
pub mod fields;
pub mod module;
pub mod names;
pub mod output;
pub mod plan;
pub mod read;
pub mod source;
pub mod yaml;
