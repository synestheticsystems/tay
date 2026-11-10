//! The engine, through the real binary: `tay up`, `tay down`, `tay status`,
//! `tay logs`, `tay ls`, `tay set`, `tay unset` and `tay restart`, the
//! daemon, what is kept on disk, the units that are made and those that
//! run, on the host and in containers. Each test runs on a machine of its
//! own, whose daemons are gone, and whose environments have nothing left in
//! Docker, when the test ends.

#[path = "../support/mod.rs"]
mod support;

mod asked;
mod commands;
mod containers;
mod daemon;
mod daemon_versions;
mod docker;
mod environments;
mod files;
mod harness;
mod one_change;
mod ports;
mod restart;
mod running;
mod secrets;
mod status;
mod up;
