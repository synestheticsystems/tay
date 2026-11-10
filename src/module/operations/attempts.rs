//! What the engine asks of a unit that runs, once its settings are filled:
//! a process, a container, a command. An attempt is driven by a future that
//! ends when the attempt does, and tells the engine each step it comes to,
//! so that the engine knows what it is doing and when the unit's own
//! program runs.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use super::{Failure, Fingerprint, UnitPlace, Values};

/// The operations of a unit that runs.
pub trait Running: Send + Sync {
    /// Begins attempt `attempt` and drives it to its end. Attempts count
    /// from 1 each time the unit is brought up.
    fn start(self: Arc<Self>, place: UnitPlace, attempt: u32, steps: Steps) -> Driving;

    /// The unit as a daemon that did not start it finds it.
    fn found(self: Arc<Self>, place: UnitPlace) -> Finding;

    /// Drives an attempt found under way from where it is to its end,
    /// telling `steps` the step it is at first.
    fn resume(self: Arc<Self>, place: UnitPlace, under_way: UnderWay, steps: Steps) -> Driving;

    /// Runs the readiness command `command` where the unit runs, for as
    /// long as `limit`.
    fn readiness_command(
        self: Arc<Self>,
        place: UnitPlace,
        command: Vec<String>,
        limit: Duration,
    ) -> ReadinessCommand;

    /// What the unit would be started from now, by its settings: what a
    /// unit that runs is compared by when it is brought up again.
    fn started_from(self: Arc<Self>, place: UnitPlace) -> Fingerprinting;
}

/// A unit being found where it runs.
pub type Finding = Pin<Box<dyn Future<Output = Result<Found, Failure>> + Send>>;

/// What a unit would be started from, by its settings, being found out.
pub type Fingerprinting = Pin<Box<dyn Future<Output = Result<Fingerprint, Failure>> + Send>>;

/// An attempt being driven: it ends with how the attempt came out.
pub type Driving = Pin<Box<dyn Future<Output = Outcome> + Send>>;

/// A readiness command being run: it ends with why it did not pass, when
/// it did not.
pub type ReadinessCommand = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Where an attempt tells the engine each step it comes to.
pub type Steps = mpsc::UnboundedSender<Step>;

/// A step of an attempt, as the engine is told it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// A step before the unit's own program, as a person reads it:
    /// `checking`, ``preparing: `bun install` ``.
    Before(String),
    /// The attempt starts the unit from these settings: its own program,
    /// or for a command, its steps. What the engine keeps of what the unit
    /// was started from.
    Started(Fingerprint),
    /// The unit's own program runs, since this time in milliseconds since
    /// 1970: its readiness is checked from now.
    Running { since: u64 },
    /// Its readiness as its module judges it, for a unit that writes no
    /// `ready` and is ready when its module says, as a container is when
    /// its image's own health check passes: it passes, in these words.
    Passes(String),
    /// It does not pass, and why.
    Fails(String),
}

/// How an attempt came out. The words of each are what a person is told,
/// read after "attempt 2": "exited with 1 at ...". They never hold a secret.
#[derive(Debug)]
pub enum Outcome {
    /// It did what it is for, as a command that is done does: it is ready,
    /// and gives these values.
    Done { values: Values, detail: String },
    /// It ended, as `detail` says: a failure unless its program exited with
    /// 0. Its restart policy decides what follows. `exit_status` is how its
    /// own program ended, as a shell gives it, when it ended by itself.
    Ended {
        failed: bool,
        detail: String,
        exit_status: Option<i64>,
    },
    /// It cannot become ready without a change, whatever its restart policy
    /// says.
    Failed(String),
}

/// A unit that runs, as a daemon that did not start it finds it.
#[derive(Debug)]
pub enum Found {
    /// Nothing of it has been started here.
    Never,
    /// An attempt is under way, under something that outlived the daemon
    /// that started it.
    UnderWay(UnderWay),
    /// Its last attempt ended, and how is known.
    Ended { attempt: u32, outcome: Outcome },
    /// Its last attempt ended with no record of how: what was left of it is
    /// to be stopped.
    Lost { attempt: u32 },
}

/// An attempt found under way: which, and the step it is at, in the words
/// of the module that drives it.
#[derive(Clone, Debug)]
pub struct UnderWay {
    pub attempt: u32,
    pub step: String,
}
