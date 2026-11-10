//! Printing a plan, or the errors that stop one. Each printer is a view over
//! the plan; what both say alike is here.

pub mod json;
pub mod text;

use crate::definition::Check;
use crate::module::ReadyWithoutCheck;
use crate::value::Value;

/// How a unit with no `ready` is known to be ready, as a person reads it.
fn ready_without_check(ready: ReadyWithoutCheck) -> &'static str {
    match ready {
        ReadyWithoutCheck::Started => "when it has started",
        ReadyWithoutCheck::Healthy => {
            "when its image's own health check passes, or when it has started if the image has none"
        }
        ReadyWithoutCheck::Exited => "when it exits with 0",
        ReadyWithoutCheck::Held => "when it is held",
        ReadyWithoutCheck::Generated => "when it has been generated",
        ReadyWithoutCheck::Written => "when it has been written",
    }
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
