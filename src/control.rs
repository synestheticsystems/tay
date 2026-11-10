//! Controlling an environment: its daemon, the commands' side of it, what
//! they say to each other over the daemon's socket, and the times they
//! record and write.

pub mod client;
pub mod clock;
pub mod daemon;
pub mod protocol;
