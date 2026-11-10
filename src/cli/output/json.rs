//! The plan for programs, in a versioned schema. A change to any key or its
//! meaning is a new schema number.

use serde_json::{Value, json};

use crate::definition::read::THIS_TAY;
use crate::diagnostic::Diagnostic;
use crate::diagnostic::source::{Loc, Source, Sources};
use crate::plan::{
    Because, Exact, GivenLine, Mechanism, OpenedBy, Plan, PlannedUnit, PublishedPort, Readiness,
    Running, Seen, Shown,
};

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
    let (module, way, made, runs, runs_inside) = match &unit.mechanism {
        Mechanism::Group => (None, None, false, None, None),
        Mechanism::Module { module, way, runs } => {
            let (runs, inside) = match runs {
                None => (None, None),
                Some(Running::OnHost) => (Some("host"), None),
                Some(Running::OwnContainer) => (Some("own_container"), None),
                Some(Running::InsideUnit(other)) => (Some("inside_unit"), Some(other.as_str())),
                Some(Running::OneOffContainer) => (Some("one_off_container"), None),
            };
            (Some(*module), way.as_deref(), runs.is_none(), runs, inside)
        }
    };
    json!({
        "name": unit.name,
        "doc": unit.doc,
        "group": matches!(unit.mechanism, Mechanism::Group),
        "module": module,
        "way": way,
        "made": made,
        "runs": runs,
        "runs_inside": runs_inside,
        "needs": unit.needs.iter().map(|need| match &need.because {
            Because::Written => json!({ "unit": need.unit, "because": "written" }),
            Because::In => json!({ "unit": need.unit, "because": "in" }),
            Because::UsesProduced(value) => json!({ "unit": need.unit, "because": "uses_produced_value", "value": value }),
            Because::UsesMade(value) => json!({ "unit": need.unit, "because": "uses_made_value", "value": value }),
        }).collect::<Vec<_>>(),
        "after": unit.after.iter().map(|after| json!({ "unit": after.unit, "comes_up": after.comes_up })).collect::<Vec<_>>(),
        "ready": match &unit.ready {
            Readiness::Group => Value::Null,
            Readiness::Default(how) => json!({ "check": Value::Null, "without_check": how }),
            Readiness::Check { check, target, within } => json!({
                "check": check,
                "target": target.iter().map(shown).collect::<Vec<_>>(),
                "within": shown(within),
            }),
        },
        "restart": unit.restart.as_ref().map(|restart| json!({
            "on": shown(&restart.on),
            "attempts": restart.attempts.as_ref().map(shown),
        })),
        "watch": unit.watch.iter().map(shown).collect::<Vec<_>>(),
        "publishes": unit.publishes.iter().map(published).collect::<Vec<_>>(),
        "gives": unit.gives.iter().map(given).collect::<Vec<_>>(),
    })
}

fn published(published: &PublishedPort) -> Value {
    let Some(opened) = &published.opened else {
        return json!({ "container_port": published.container_port, "opened": false });
    };
    json!({
        "container_port": published.container_port,
        "opened": true,
        "address": shown(&opened.address),
        "host_port": shown(&opened.host_port),
        "always": opened.always,
        "opened_by": opened.by.iter().map(|opener| json!({
            "unit": opener.unit,
            "how": match opener.how {
                OpenedBy::UnitOnHost => "unit_on_host",
                OpenedBy::ReadinessCheck => "readiness_check",
                OpenedBy::View => "view",
            },
        })).collect::<Vec<_>>(),
    })
}

fn given(given: &GivenLine) -> Value {
    let seen = |seen: &Seen| match seen {
        Seen::Value(value) => shown(value),
        Seen::Unreachable(why) => json!({ "unreachable": why }),
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

/// A value: its text with holes shown, and its exact value or `null`.
fn shown(value: &Shown) -> Value {
    json!({
        "text": value.text,
        "value": match &value.exact {
            Some(Exact::Text(text)) => json!(text),
            Some(Exact::Number(n)) => json!(n),
            Some(Exact::Bool(b)) => json!(b),
            None => Value::Null,
        },
    })
}
