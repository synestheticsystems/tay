//! Printing a plan, or the errors that stop one, the events of a change,
//! and what the environment remembers being asked for. Each printer of a
//! plan is a view over it; what both say alike is here.

pub mod asked;
pub mod change;
pub mod json;
pub mod text;

use crate::definition::Check;
use crate::plan::Unreachable;
use crate::value::Value;

/// Why a value a unit gives has no form for the host, as a person reads it.
fn unreachable(why: &[Unreachable]) -> String {
    let each: Vec<String> = why
        .iter()
        .map(|why| match why {
            Unreachable::NotPublished { port } => {
                format!("its port {port} is not published to the host")
            }
            Unreachable::NotOpened { port } => format!(
                "its port {port} is published, but nothing in the plan opens it on the host"
            ),
        })
        .collect();
    each.join("; ")
}

/// The kind of a readiness check, as `ready` writes it, and what it checks.
fn check(check: &Check<Value>) -> (&'static str, Vec<&Value>) {
    match check {
        Check::Http(url) => ("http", vec![url]),
        Check::Tcp(address) => ("tcp", vec![address]),
        Check::Command(words) => ("command", words.iter().collect()),
        Check::Log(line) => ("log", vec![line]),
    }
}
