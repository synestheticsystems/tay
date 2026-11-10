//! What goes into and out of a container: a program run inside it, a file
//! fetched from it, and its output, copied into its unit's log.

mod archive;
mod exec;
mod logs;

pub use exec::{Exec, Exited};
pub use logs::copy_output;
