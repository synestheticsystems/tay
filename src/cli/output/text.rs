//! The plan as a person reads it.

use crate::module::{RestartOn, Runs, UnitKind};
use crate::plan::{
    Because, Given, OpenedBy, Plan, PlannedMechanism, PlannedUnit, PublishedPort, Readiness, Seen,
};

pub fn plan(plan: &Plan) -> String {
    let profile = match (&plan.profile, plan.production) {
        (Some(profile), true) => format!("profile `{profile}` (production)"),
        (Some(profile), false) => format!("profile `{profile}`"),
        (None, _) => "no profile".to_string(),
    };
    let mut lines = vec![format!(
        "Plan for stack `{}`, {profile}, in {}",
        plan.stack, plan.root
    )];
    lines.push(match plan.units.len() {
        0 => "Nothing comes up: `up` names no unit.".to_string(),
        1 => "1 unit comes up.".to_string(),
        n => format!("{n} units come up, in an order they could start in."),
    });
    for (i, unit) in plan.units.iter().enumerate() {
        lines.push(String::new());
        unit_lines(&mut lines, i + 1, unit);
    }
    lines.push(String::new());
    lines.join("\n")
}

fn unit_lines(lines: &mut Vec<String>, number: usize, unit: &PlannedUnit) {
    let what = match &unit.mechanism {
        None => "a group".to_string(),
        Some(mechanism) => {
            let way = mechanism
                .way
                .as_ref()
                .map(|way| format!(" (way `{way}`)"))
                .unwrap_or_default();
            format!(
                "uses `{}`{way}, {}",
                mechanism.module.name(),
                kind(&mechanism.kind)
            )
        }
    };
    lines.push(format!("{number}. {}: {what}", unit.name));
    if let Some(doc) = &unit.doc {
        lines.push(format!("   {doc}"));
    }
    for need in &unit.needs {
        let unit = &need.unit;
        lines.push(match &need.because {
            Because::Written => format!("   needs {unit}"),
            Because::In => format!("   needs {unit}, because it runs inside its container"),
            Because::UsesProduced(value) => format!(
                "   needs {unit}, because it uses {value}, which {unit} produces by running"
            ),
            Because::UsesMade(value) => format!("   needs {unit}, because it uses {value}"),
        });
    }
    for after in &unit.after {
        let comes_up = if after.comes_up {
            ""
        } else {
            ", which does not come up"
        };
        lines.push(format!("   waits for {} (after){comes_up}", after.unit));
    }
    if let Some(mechanism) = &unit.mechanism {
        mechanism_lines(lines, mechanism);
    }
    for given in &unit.gives {
        given_lines(lines, given);
    }
}

fn mechanism_lines(lines: &mut Vec<String>, mechanism: &PlannedMechanism) {
    lines.push(match &mechanism.ready {
        Readiness::WithoutCheck(ready) => {
            format!("   ready: {}", ready.words())
        }
        Readiness::Check { check, within } => {
            let (kind, target) = super::check(check);
            let target: Vec<String> = target.iter().map(|value| value.shown()).collect();
            format!(
                "   ready: {kind} check of {}, within {within}",
                target.join(" ")
            )
        }
    });
    let restart = &mechanism.restart;
    lines.push(match restart.on {
        RestartOn::Never => "   restart: never".to_string(),
        on => {
            let attempts = restart
                .attempts
                .map(|attempts| format!(", up to {attempts} attempts"))
                .unwrap_or_default();
            format!("   restart: on {}{attempts}", on.word())
        }
    });
    for folder in &mechanism.watch {
        lines.push(format!("   watches {}", folder.shown()));
    }
    for published in &mechanism.publishes {
        lines.push(published_line(published));
    }
}

fn published_line(published: &PublishedPort) -> String {
    let port = published.container_port;
    let Some(opened) = &published.opened else {
        return format!(
            "   publishes {port}: not opened, since nothing in the plan uses it from the host"
        );
    };
    let mut by: Vec<String> = opened
        .by
        .iter()
        .map(|opener| match opener.how {
            OpenedBy::UnitOnHost => format!("{} (on the host)", opener.unit),
            OpenedBy::ReadinessCheck => format!("{} (its readiness check)", opener.unit),
            OpenedBy::View => format!("{} (through a view)", opener.unit),
        })
        .collect();
    if opened.always {
        by.insert(0, "always".to_string());
    }
    format!(
        "   publishes {port} at {}:{}, opened for {}",
        opened.address.shown(),
        opened.host_port.shown(),
        by.join(", ")
    )
}

fn kind(kind: &UnitKind) -> String {
    match kind {
        UnitKind::Made => "is made".to_string(),
        UnitKind::Runs(Runs::OnHost) => "runs on the host".to_string(),
        UnitKind::Runs(Runs::OwnContainer) => "runs in its own container".to_string(),
        UnitKind::Runs(Runs::InsideUnit(unit)) => {
            format!("runs inside the container of `{}`", unit.name)
        }
        UnitKind::Runs(Runs::OneOffContainer) => "runs in a container made for the run".to_string(),
    }
}

fn given_lines(lines: &mut Vec<String>, given: &Given) {
    let mut notes = Vec::new();
    if given.secret {
        notes.push("secret".to_string());
    }
    if !given.producers.is_empty() {
        notes.push(format!(
            "produced by running {}",
            given.producers.join(", ")
        ));
    }
    let notes = if notes.is_empty() {
        String::new()
    } else {
        format!(" ({})", notes.join("; "))
    };
    let seen = |seen: &Seen| match seen {
        Seen::Value(value) => value.shown(),
        Seen::Unreachable(why) => format!("not reachable: {}", super::unreachable(why)),
    };
    let (host, containers) = (seen(&given.from_host), seen(&given.from_containers));
    if host == containers {
        lines.push(format!("   gives {} = {host}{notes}", given.name));
    } else {
        lines.push(format!("   gives {}{notes}", given.name));
        lines.push(format!("     from the host: {host}"));
        lines.push(format!("     from a container: {containers}"));
    }
}
