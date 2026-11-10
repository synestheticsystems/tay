//! What the commands and an environment's daemon say to each other: HTTP
//! with JSON bodies over the daemon's Unix socket, under `/v1/`. A change
//! answers with its events as they happen, one JSON object to a line.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::disk::{self, Asked};
use crate::engine::{Decision, Left, State};
use crate::plan::Options;

/// `GET`: who the daemon is, its [`About`]. Every version of tay answers it
/// alike, so that a command can tell a daemon of another version.
pub const DAEMON: &str = "/v1/daemon";
/// `POST` an [`UpRequest`]: brings units up, answering with [`Event`]s.
pub const UP: &str = "/v1/up";
/// `POST`: stops every unit, answering with [`Event`]s, and the daemon
/// exits.
pub const DOWN: &str = "/v1/down";
/// `GET`: each unit's state from a check, a [`Status`].
pub const STATUS: &str = "/v1/status";
/// `POST`: the daemon exits and leaves the stack as it is, for a command of
/// another version that starts its own. It answers `{}` first.
pub const EXIT: &str = "/v1/exit";

/// Who a daemon is.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct About {
    /// The version of tay it is.
    pub tay: String,
    pub pid: u32,
    /// The environment it is for: its stack's root and its id.
    pub root: PathBuf,
    pub id: String,
    /// When it started, in seconds since 1970.
    pub started: u64,
}

/// What `tay up` asks for: what it plans with, exactly as `tay plan` would,
/// which the environment remembers. It has no `Debug`, since a `--set` may
/// give a secret setting its value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpRequest {
    /// The profile, the `--set`s and the units, what the environment
    /// remembered with what this `tay up` adds.
    #[serde(flatten)]
    pub asked: Asked,
    /// The home folder of whoever asks, where their own choices are.
    pub home: Option<PathBuf>,
}

impl UpRequest {
    /// What the daemon plans with: what was asked, and this machine's
    /// folder of shared folders.
    pub fn options(&self) -> Options {
        Options {
            asked: self.asked.clone(),
            home: self.home.clone(),
            shared_folders: disk::shared_folders(),
        }
    }
}

/// One thing that happens during a change, as one line of its answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The units the change acts on, in the order they start in.
    Units { units: Vec<String> },
    /// A unit took a state, at a time in seconds since 1970.
    State {
        unit: String,
        state: State,
        detail: String,
        at: u64,
    },
    /// Something a person should know of the change, said once.
    Notice { notice: String },
    /// The definitions, planned again by the daemon, have errors: each as
    /// `tay plan` prints it.
    Errors { lines: Vec<String>, too_old: bool },
    /// What `tay up` came to.
    Up(Decision),
    /// `tay up` was stopped before it came to anything, by what is
    /// said, as a person reads it.
    Cancelled { by: String },
    /// The end of `tay down`: what is left, as it could not be stopped or
    /// removed.
    Down { left: Vec<Left> },
    /// The change could not be made, and why.
    Problem { problem: String },
}

impl Event {
    /// Whether this is the last thing a change says.
    pub fn ends_change(&self) -> bool {
        match self {
            Event::Units { .. } | Event::State { .. } | Event::Notice { .. } => false,
            Event::Errors { .. }
            | Event::Up(_)
            | Event::Cancelled { .. }
            | Event::Down { .. }
            | Event::Problem { .. } => true,
        }
    }
}

/// Why a change was refused: another is running, since a time in seconds
/// since 1970. It is the body of a `409 Conflict`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Refused {
    pub change: String,
    pub since: u64,
}

/// Each unit's state, in the order units start in.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub units: Vec<UnitStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnitStatus {
    pub unit: String,
    pub state: State,
    pub detail: String,
    /// When it took its state, in seconds since 1970.
    pub since: u64,
}

/// An answer that is not what was asked for: its reason.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Problem {
    pub problem: String,
}
