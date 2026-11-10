//! What the commands and an environment's daemon say to each other: HTTP
//! with JSON bodies over the daemon's Unix socket, under `/v1/`. A change
//! answers with its events as they happen, one JSON object to a line.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::disk::{Asked, DiskError, Layout};
use crate::engine::{Decision, Left, State};
use crate::plan::ActionRun;

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
/// `POST` a [`Remember`]: changes what the environment remembers being
/// asked for, and touches no unit. It answers `{}`.
pub const REMEMBER: &str = "/v1/remember";
/// `POST` a [`Restart`]: stops units and starts them again, answering with
/// [`Event`]s.
pub const RESTART: &str = "/v1/restart";
/// `POST` a [`RunRequest`]: runs an action, answering with [`Event`]s:
/// each change of state, each line the action writes, and what it came to.
pub const RUN: &str = "/v1/run";
/// `POST` a [`WipeRequest`]: removes what units keep, or everything the
/// environment keeps, answering with [`Event`]s.
pub const WIPE: &str = "/v1/wipe";

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

/// What `tay up` asks for: what its own command line adds to what the
/// environment remembers being asked for. The daemon joins the two once it
/// holds the place of the change under way, so that a `tay set` made after
/// the command read what is remembered is not lost. It has no `Debug`,
/// since a `--set` may give a secret setting its value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpRequest {
    /// The profile, the `--set`s and the units this `tay up` names.
    #[serde(flatten)]
    pub adds: Asked,
    /// The home folder of whoever asks, where their own choices are.
    pub home: Option<PathBuf>,
}

impl UpRequest {
    /// What is asked once this request is joined with what is
    /// `remembered`.
    pub fn joined(&self, remembered: Asked) -> Asked {
        let adds = &self.adds;
        remembered.with(adds.profile.clone(), &adds.sets, &adds.units)
    }
}

/// A change to what the environment remembers being asked for, which no
/// unit sees until the next `tay up`. It has no `Debug`, since a `tay set`
/// may give a secret setting its value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Remember {
    /// `tay set`: each `PLACE=VALUE` replaces what is remembered for its
    /// place, as a `--set` of `tay up` does.
    Set { sets: Vec<String> },
    /// `tay unset`: what is remembered for each place is forgotten.
    Unset { places: Vec<String> },
}

impl Remember {
    /// Makes the change to what the environment at `layout` remembers.
    pub fn apply(&self, layout: &Layout) -> Result<(), DiskError> {
        let remembered = Asked::read(layout)?;
        let asked = match self {
            Remember::Set { sets } => remembered.with(None, sets, &[]),
            Remember::Unset { places } => remembered.without(places),
        };
        asked.write(layout)
    }

    /// The change as the command that asked for it is written, with no
    /// value, which may be a secret.
    pub fn describe(&self) -> String {
        let words: Vec<String> = match self {
            Remember::Set { sets } => std::iter::once("tay set".to_string())
                .chain(sets.iter().map(|set| format!("{}=…", Asked::place(set))))
                .collect(),
            Remember::Unset { places } => std::iter::once("tay unset".to_string())
                .chain(places.iter().cloned())
                .collect(),
        };
        words.join(" ")
    }
}

/// What `tay run` asks for: the action and its arguments, which come into
/// the plan beside what the environment remembers being asked for, and the
/// home folder of whoever asks, where their own choices are.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRequest {
    #[serde(flatten)]
    pub run: ActionRun,
    pub home: Option<PathBuf>,
}

/// What `tay wipe --all` removes of the environment's folder, as a person
/// reads it.
pub const FOLDER_WIPED: &str = "`.tay/` but for `choices.yaml`";

/// What `tay wipe --all` removes of the machine's list, as a person reads
/// it.
pub const LISTING_WIPED: &str = "its place on this machine's list, with the ports it kept";

/// What `tay wipe --yes` asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "wipe", rename_all = "snake_case")]
pub enum WipeRequest {
    /// What the units named keep, and then, on an environment meant to be
    /// up, everything brought up again with the choices of whoever has
    /// `home`.
    Units {
        units: Vec<String>,
        home: Option<PathBuf>,
    },
    /// Everything the environment keeps, once it is taken down.
    All,
}

/// What `tay restart` asks for: the units to stop and start again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restart {
    pub units: Vec<String>,
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
    /// What a change that brings units up came to: `tay up`, or another
    /// that ends by bringing up what it stopped.
    Decided(Decision),
    /// `tay up` was stopped before it came to anything, by what is
    /// said, as a person reads it.
    Cancelled { by: String },
    /// The end of `tay down`: what is left, as it could not be stopped or
    /// removed.
    Down { left: Vec<Left> },
    /// The change could not be made, and why.
    Problem { problem: String },
    /// A line the action that is run wrote, as it wrote it.
    Output { line: String },
    /// What a unit kept, or with no unit what the environment kept, has
    /// been removed: each thing, as a person reads it.
    Removed {
        unit: Option<String>,
        removed: Vec<String>,
    },
    /// The end of `tay wipe` on an environment that is down, or of
    /// `tay wipe --all`: what was asked is removed, and nothing is brought
    /// up.
    Wiped,
    /// The change was refused as things stand, once the daemon could tell,
    /// and nothing of it was done.
    Refused(Refused),
    /// The end of `tay run`: what bringing the action up came to, how its
    /// own program exited when it came to an end, as a shell gives the exit
    /// status, and the values it produced.
    Ran {
        decision: Decision,
        exit_status: Option<i64>,
        values: Vec<Produced>,
    },
}

/// A value an action produced, by name: `None` for one declared secret,
/// which is never given.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Produced {
    pub name: String,
    pub value: Option<String>,
}

impl Event {
    /// Whether this is the last thing a change says.
    pub fn ends_change(&self) -> bool {
        match self {
            Event::Units { .. }
            | Event::State { .. }
            | Event::Notice { .. }
            | Event::Output { .. }
            | Event::Removed { .. } => false,
            Event::Errors { .. }
            | Event::Decided(_)
            | Event::Cancelled { .. }
            | Event::Down { .. }
            | Event::Problem { .. }
            | Event::Refused(_)
            | Event::Ran { .. }
            | Event::Wiped => true,
        }
    }
}

/// Why a change was refused as things stand, so that the same request may
/// be made later: the body of a `409 Conflict`. A request that does not fit
/// the stack is answered `422 Unprocessable Entity` with a [`Problem`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "refused", rename_all = "snake_case")]
pub enum Refused {
    /// Another change is running, since a time in seconds since 1970.
    AnotherChange { change: String, since: u64 },
    /// What was asked needs something that is not up, as said.
    NotUp { problem: String },
    /// What was asked could be done only by replacing a unit that is up,
    /// as said.
    WouldReplace { problem: String },
}

impl Refused {
    /// A change that brings up units on an environment that is down.
    pub fn environment_down() -> Refused {
        Refused::NotUp {
            problem: "the environment is down: `tay up` brings it up".to_string(),
        }
    }
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::AnotherChange { change, since } => write!(
                f,
                "another change is running: `{change}`, since {}",
                crate::clock::utc(*since)
            ),
            Refused::NotUp { problem } | Refused::WouldReplace { problem } => f.write_str(problem),
        }
    }
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
