//! What the engine asks of a unit once its settings are filled. Every
//! module answers the same few operations, which differ only by whether its
//! units are made or run, so the engine drives each unit without knowing its
//! module.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::disk::Environment;
use crate::value::{Exact, Filled};

/// The operations of one unit, from its filled settings.
pub enum Operations {
    /// A unit that is made and then simply exists.
    Made(Box<dyn Made>),
    /// A unit that runs.
    Running(Arc<dyn Running>),
}

/// Where a module acts for a unit: the unit, and the environment it is in.
#[derive(Clone)]
pub struct UnitPlace {
    pub unit: String,
    pub environment: Arc<Environment>,
}

/// The operations of a unit that is made: a port held, a secret generated,
/// a file written.
pub trait Made: Send + Sync {
    /// Makes the unit, doing nothing when it already exists and is right.
    /// `in_use` is whether a unit of the environment that uses its values
    /// runs now, and so may hold what it made, as a process holds the port
    /// it was given.
    fn make(&self, place: &UnitPlace, in_use: bool) -> Result<Exists, Failure>;

    /// The unit as it exists now, from a check made now; why not, when it
    /// does not exist or is not right.
    fn check(&self, place: &UnitPlace) -> Result<Exists, Failure>;
}

/// A unit that is made, as it exists and is right: the values it gives,
/// and what a person is told about it, which never holds a secret.
#[derive(Debug)]
pub struct Exists {
    pub values: Values,
    pub detail: String,
}

/// The values a unit gives, by name.
pub type Values = BTreeMap<String, Filled>;

/// The operations of a unit that runs, from its filled settings: a process,
/// a command. An attempt is driven by a future that ends when the attempt
/// does; it tells the engine each step it comes to, so that the engine knows
/// what it is doing and when the unit's own program runs.
pub trait Running: Send + Sync {
    /// Begins attempt `attempt` and drives it to its end. `prepare` is
    /// whether the unit prepares first, as a process does before the first
    /// attempt each time it is brought up.
    fn start(
        self: Arc<Self>,
        place: UnitPlace,
        attempt: u32,
        prepare: bool,
        steps: Steps,
    ) -> Driving;

    /// The unit as a daemon that did not start it finds it.
    fn found(&self, place: &UnitPlace) -> Result<Found, Failure>;

    /// Drives an attempt found under way from where it is to its end.
    fn resume(self: Arc<Self>, place: UnitPlace, under_way: UnderWay, steps: Steps) -> Driving;

    /// Runs the readiness command `command` where the unit runs, for as
    /// long as `limit`; why it did not pass, when it did not.
    fn check(self: Arc<Self>, place: UnitPlace, command: Vec<String>, limit: Duration) -> Checking;
}

/// An attempt being driven: it ends with how the attempt came out.
pub type Driving = Pin<Box<dyn Future<Output = Outcome> + Send>>;

/// A readiness command being run: it ends with why it did not pass, when
/// it did not.
pub type Checking = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Where an attempt tells the engine each step it comes to.
pub type Steps = mpsc::UnboundedSender<Step>;

/// A step of an attempt, as the engine is told it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// A step before the unit's own program, as a person reads it:
    /// `checking`, ``preparing: `bun install` ``.
    Before(String),
    /// The unit's own program runs, since this time in milliseconds since
    /// 1970: its readiness is checked from now.
    Running { since: u64 },
}

/// How an attempt came out. The words of each are what a person is told,
/// read after "attempt 2": "exited with 1 at ...". They never hold a secret.
#[derive(Debug)]
pub enum Outcome {
    /// It did what it is for, as a command that is done does: it is ready,
    /// and gives these values.
    Done { values: Values, detail: String },
    /// It ended, as `detail` says: a failure unless its program exited with
    /// 0. Its restart policy decides what follows.
    Ended { failed: bool, detail: String },
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

/// An attempt found under way: which, the step it is at, and where the
/// module that drives it resumes, in that module's own words.
#[derive(Clone, Debug)]
pub struct UnderWay {
    pub attempt: u32,
    pub step: Step,
    pub resumes_at: String,
}

/// Why a module could not do what was asked for a unit. Its words are the
/// reason a person is given, and never hold a secret.
#[derive(Debug)]
pub struct Failure(Box<dyn Error + Send + Sync>);

impl<E: Error + Send + Sync + 'static> From<E> for Failure {
    fn from(error: E) -> Failure {
        Failure(Box::new(error))
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A setting filled with a value of another type than its module reads it
/// as. The planner checks every value whose type is known before start, so
/// only a value produced by running can be one.
#[derive(Debug, thiserror::Error)]
#[error("`{setting}` was filled with {found}, and {wanted} is wanted")]
pub struct WrongType {
    setting: &'static str,
    wanted: &'static str,
    found: &'static str,
}

/// `setting`'s filled value as a whole number.
pub fn number(value: &Filled, setting: &'static str) -> Result<i64, WrongType> {
    match value.plain() {
        Some(Exact::Number(n)) => Ok(*n),
        _ => Err(wrong(value, setting, "a whole number")),
    }
}

/// `setting`'s filled value as text with no secret in it.
pub fn text<'v>(value: &'v Filled, setting: &'static str) -> Result<&'v str, WrongType> {
    match value.plain() {
        Some(Exact::Text(text)) => Ok(text),
        _ => Err(wrong(value, setting, "text")),
    }
}

/// `setting`'s filled value as true or false.
pub fn boolean(value: &Filled, setting: &'static str) -> Result<bool, WrongType> {
    match value.plain() {
        Some(Exact::Bool(b)) => Ok(*b),
        _ => Err(wrong(value, setting, "true or false")),
    }
}

fn wrong(value: &Filled, setting: &'static str, wanted: &'static str) -> WrongType {
    let found = match value.plain() {
        None => "a secret",
        Some(Exact::Text(_)) => "text",
        Some(Exact::Number(_)) => "a whole number",
        Some(Exact::Bool(_)) => "true or false",
    };
    WrongType {
        setting,
        wanted,
        found,
    }
}
