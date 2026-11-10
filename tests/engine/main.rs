//! The engine, through the real binary: `tay up`, `tay down`, `tay status`
//! and `tay ls`, the daemon, what is kept on disk, and the units that are
//! made. Each test runs on a machine of its own, whose daemons are gone
//! when the test ends.

#[path = "../support/mod.rs"]
mod support;

mod asked;
mod commands;
mod daemon;
mod daemon_versions;
mod environments;
mod files;
mod harness;
mod one_change;
mod ports;
mod secrets;
mod status;
mod up;
