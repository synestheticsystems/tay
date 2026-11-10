//! Units that run on the host, through the real binary: each program under
//! a keeper that outlives the daemon, found again by the next daemon, its
//! readiness checked and its restarts made, stopped with all it started,
//! commands run once with what they produce, and their logs.

mod commands;
mod environment;
mod finding_again;
mod keeper;
mod logs;
mod readiness;
mod restarts;
mod secrets;
mod stopping;
