//! The plan for programs, in a versioned schema. A change to any key or its
//! meaning is a new schema number.

use serde_json::{Value, json};

use crate::definition::read::THIS_TAY;
use crate::diagnostic::Diagnostic;
use crate::diagnostic::source::{Loc, Source, Sources};
use crate::module::{Runs, UnitKind};
use crate::plan::{Because, Given, OpenedBy, Plan, PlannedUnit, PublishedPort, Readiness, Seen};
use crate::value::Exact;

pub const SCHEMA: u32 = 1;

pub fn plan(plan: &Plan) -> Value {
    json!({
        "schema": SCHEMA,
        "tay": THIS_TAY,
        "outcome": "plan",
        "plan": {
            "stack": plan.stack,
            "root": plan.root,
            "profile": plan.profile,
            "production": plan.production,
            "units": plan.units.iter().map(unit).collect::<Vec<_>>(),
        },
    })
}

/// `outcome` is `errors` (the definitions have errors) or `too_old` (this
/// tay may not read them).
pub fn errors(outcome: &str, errors: &[Diagnostic], sources: &Sources) -> Value {
    json!({
        "schema": SCHEMA,
        "tay": THIS_TAY,
        "outcome": outcome,
        "errors": errors.iter().map(|error| json!({
            "at": at(error.loc, sources),
            "message": error.message,
            "related": error.related.iter().map(|related| json!({
                "at": at(related.loc, sources),
                "message": related.message,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// An error about no place in the definitions, such as finding no stack.
pub fn error_without_place(message: &str) -> Value {
    json!({
        "schema": SCHEMA,
        "tay": THIS_TAY,
        "outcome": "errors",
        "errors": [{ "at": Value::Null, "message": message, "related": [] }],
    })
}

fn at(loc: Loc, sources: &Sources) -> Value {
    match sources.get(loc.source) {
        Source::File { shown, .. } => {
            json!({ "file": shown, "line": loc.line, "column": loc.column })
        }
        Source::Argument { shown } => json!({ "argument": shown }),
    }
}

fn unit(unit: &PlannedUnit) -> Value {
    let mechanism = unit.mechanism.as_ref();
    let (runs, runs_inside) = match mechanism.map(|mechanism| &mechanism.kind) {
        None | Some(UnitKind::Made) => (None, None),
        Some(UnitKind::Runs(Runs::OnHost)) => (Some("host"), None),
        Some(UnitKind::Runs(Runs::OwnContainer)) => (Some("own_container"), None),
        Some(UnitKind::Runs(Runs::InsideUnit(other))) => {
            (Some("inside_unit"), Some(other.name.as_str()))
        }
        Some(UnitKind::Runs(Runs::OneOffContainer)) => (Some("one_off_container"), None),
    };
    json!({
        "name": unit.name,
        "doc": unit.doc,
        "group": mechanism.is_none(),
        "module": mechanism.map(|mechanism| mechanism.module.name()),
        "way": mechanism.and_then(|mechanism| mechanism.way.as_deref()),
        "made": mechanism.is_some_and(|mechanism| matches!(mechanism.kind, UnitKind::Made)),
        "runs": runs,
        "runs_inside": runs_inside,
        "needs": unit.needs.iter().map(|need| match &need.because {
            Because::Written => json!({ "unit": need.unit, "because": "written" }),
            Because::In => json!({ "unit": need.unit, "because": "in" }),
            Because::UsesProduced(value) => json!({ "unit": need.unit, "because": "uses_produced_value", "value": value }),
            Because::UsesMade(value) => json!({ "unit": need.unit, "because": "uses_made_value", "value": value }),
        }).collect::<Vec<_>>(),
        "after": unit.after.iter().map(|after| json!({ "unit": after.unit, "comes_up": after.comes_up })).collect::<Vec<_>>(),
        "ready": mechanism.map(|mechanism| match &mechanism.ready {
            Readiness::WithoutCheck(ready) => json!({
                "check": Value::Null,
                "without_check": ready.words(),
            }),
            Readiness::Check { check, within } => {
                let (kind, target) = super::check(check);
                json!({
                    "check": kind,
                    "target": target.into_iter().map(shown).collect::<Vec<_>>(),
                    "within": word(&within.to_string()),
                })
            }
        }),
        "restart": mechanism.map(|mechanism| json!({
            "on": word(mechanism.restart.on.word()),
            "attempts": mechanism.restart.attempts.map(|attempts| json!({
                "text": attempts.to_string(),
                "value": attempts,
            })),
        })),
        "watch": mechanism.map_or(Vec::new(), |mechanism| mechanism.watch.iter().map(shown).collect()),
        "publishes": mechanism.map_or(Vec::new(), |mechanism| mechanism.publishes.iter().map(published_port).collect()),
        "gives": unit.gives.iter().map(given).collect::<Vec<_>>(),
    })
}

fn published_port(published: &PublishedPort) -> Value {
    let Some(opening) = &published.opening else {
        return json!({ "container_port": published.container_port, "opened": false });
    };
    json!({
        "container_port": published.container_port,
        "opened": true,
        "address": shown(&opening.address),
        "host_port": shown(&opening.host_port),
        "always": opening.always,
        "opened_by": opening.openers.iter().map(|opener| json!({
            "unit": opener.unit,
            "how": match opener.how {
                OpenedBy::UnitOnHost => "unit_on_host",
                OpenedBy::ReadinessCheck => "readiness_check",
                OpenedBy::View => "view",
            },
        })).collect::<Vec<_>>(),
    })
}

fn given(given: &Given) -> Value {
    let seen = |seen: &Seen| match seen {
        Seen::Value(value) => shown(value),
        Seen::Unreachable(why) => json!({ "unreachable": super::unreachable(why) }),
    };
    json!({
        "name": given.name,
        "endpoint": given.endpoint,
        "known_before_start": given.producers.is_empty(),
        "produced_by": given.producers,
        "secret": given.secret,
        "from_host": seen(&given.from_host),
        "from_containers": seen(&given.from_containers),
    })
}

/// A value: its text with placeholders shown, and its exact value, `null`
/// when it has a placeholder.
fn shown(value: &crate::value::Value) -> Value {
    json!({
        "text": value.shown(),
        "value": match value.exact() {
            Some(Exact::Text(text)) => json!(text),
            Some(Exact::Number(n)) => json!(n),
            Some(Exact::Bool(b)) => json!(b),
            None => Value::Null,
        },
    })
}

/// A word that is its own value, such as `restart.on`.
fn word(word: &str) -> Value {
    json!({ "text": word, "value": word })
}
