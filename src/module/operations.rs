//! What the engine asks of a unit once its settings are filled. Every
//! module answers the same few operations, which differ only by whether its
//! units are made or run, so the engine drives each unit without knowing its
//! module.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use crate::disk::Environment;
use crate::value::{Exact, Filled};

/// The operations of one unit, from its filled settings.
pub enum Operations {
    /// A unit that is made and then simply exists.
    Made(Box<dyn Made>),
    /// A unit that runs.
    Running(Box<dyn Running>),
}

/// Where a module acts for a unit: the unit, and the environment it is in.
pub struct UnitPlace<'a> {
    pub unit: &'a str,
    pub environment: &'a Environment,
}

/// The operations of a unit that is made: a port held, a secret generated,
/// a file written.
pub trait Made: Send + Sync {
    /// Makes the unit, doing nothing when it already exists and is right.
    fn make(&self, place: &UnitPlace<'_>) -> Result<Exists, Failure>;

    /// The unit as it exists now, from a check made now; why not, when it
    /// does not exist or is not right.
    fn check(&self, place: &UnitPlace<'_>) -> Result<Exists, Failure>;
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

/// The operations of a unit that runs: a process, a container, a command.
pub trait Running: Send + Sync {
    /// Begins an attempt.
    fn start(&self, place: &UnitPlace<'_>) -> Result<Attempt, Failure>;

    /// Finds the unit's attempt again, for a daemon that did not start it.
    fn find(&self, place: &UnitPlace<'_>) -> Result<Attempt, Failure>;

    /// How the unit's attempt ended, once it has; `None` while it runs.
    fn ended(&self, place: &UnitPlace<'_>) -> Result<Option<Ending>, Failure>;

    /// Stops the unit's attempt and whatever it started in turn.
    fn stop(&self, place: &UnitPlace<'_>) -> Result<(), Failure>;
}

/// An attempt of a unit that runs, under way. No module starts one yet,
/// so there is none: the keeper, which runs host processes and commands,
/// and the containers are the next parts of the engine.
pub enum Attempt {}

/// How an attempt ended: its exit code or signal. There is none until a
/// module starts attempts.
pub enum Ending {}

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
