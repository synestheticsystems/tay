//! What the engine asks of a unit once its settings are filled. Every
//! module answers the same few operations, which differ only by whether its
//! units are made or run, so the engine drives each unit without knowing its
//! module.

mod attempts;

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use crate::disk::Environment;
use crate::value::{Exact, Filled};

pub use attempts::{
    Comparing, Driving, Finding, Found, Outcome, ReadinessCommand, Running, Step, Steps, UnderWay,
};

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

impl UnitPlace {
    /// Where another unit of the same environment is acted on.
    pub fn of(&self, unit: &str) -> UnitPlace {
        UnitPlace {
            unit: unit.to_string(),
            environment: Arc::clone(&self.environment),
        }
    }
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

/// `setting`'s filled value as one word, text, a number or true or false,
/// with no secret in it.
pub fn scalar(value: &Filled, setting: &'static str) -> Result<String, WrongType> {
    match value.plain() {
        Some(exact) => Ok(exact.as_text()),
        None => Err(wrong(value, setting, "text, a number or true or false")),
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
