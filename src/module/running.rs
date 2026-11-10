//! The modules whose units run something: a process on the host, a container,
//! and a command run once; and the settings they share, each module still
//! deciding which of these it has.

pub mod command;
mod command_line;
pub mod container;
mod mounts;
pub mod process;
mod variables;
