//! The engine, through the real binary: `tay up`, `tay down`, `tay status`,
//! `tay logs` and `tay ls`, the daemon, what is kept on disk, the units that
//! are made and those that run. Each test runs on a machine of its own, whose daemons are gone
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
mod running;
mod secrets;
mod status;
mod up;
