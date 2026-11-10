//! Controlling an environment: its daemon, the commands' side of it, and
//! what they say to each other over the daemon's socket.

pub mod client;
pub mod daemon;
pub mod protocol;
