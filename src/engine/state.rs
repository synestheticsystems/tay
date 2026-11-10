//! A unit's state, as `docs/engine.md` defines each.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Something it needs is not ready yet.
    Waiting,
    /// An attempt is under way.
    Starting,
    /// It exists, for a unit that is made; its readiness check passes, for
    /// one that runs.
    Ready,
    /// It cannot become ready without a change.
    Failed,
    /// It is not running because it was stopped or never asked for.
    Stopped,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Starting => "starting",
            State::Ready => "ready",
            State::Failed => "failed",
            State::Stopped => "stopped",
        }
    }
}

/// A unit's state and what a person is told about it: what it waits for,
/// what its attempt is doing, why it failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitState {
    pub state: State,
    pub detail: String,
}

impl UnitState {
    pub fn new(state: State, detail: impl Into<String>) -> UnitState {
        UnitState {
            state,
            detail: detail.into(),
        }
    }
}
