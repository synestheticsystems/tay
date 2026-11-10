//! The plan as a person reads it.

use crate::plan::{Because, GivenLine, Mechanism, Plan, PlannedUnit, Readiness, Running, Seen};

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
    let mechanism = match &unit.mechanism {
        Mechanism::Group => "a group".to_string(),
        Mechanism::Module { module, way, runs } => {
            let way = way
                .as_ref()
                .map(|way| format!(" (way `{way}`)"))
                .unwrap_or_default();
            let runs = match runs {
                Some(runs) => running(runs),
                None => "is made".to_string(),
            };
            format!("uses `{module}`{way}, {runs}")
        }
    };
    lines.push(format!("{number}. {}: {mechanism}", unit.name));
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
    match &unit.ready {
        Readiness::Group => {}
        Readiness::Default(how) => lines.push(format!("   ready: {how}")),
        Readiness::Check {
            check,
            target,
            within,
        } => {
            let target: Vec<&str> = target.iter().map(|t| t.text.as_str()).collect();
            lines.push(format!(
                "   ready: {check} check of {}, within {}",
                target.join(" "),
                within.text
            ));
        }
    }
    if let Some(restart) = &unit.restart {
        let attempts = restart
            .attempts
            .as_ref()
            .map(|a| format!(", up to {} attempts", a.text))
            .unwrap_or_default();
        lines.push(match restart.on.text.as_str() {
            "never" => "   restart: never".to_string(),
            on => format!("   restart: on {on}{attempts}"),
        });
    }
    for folder in &unit.watch {
        lines.push(format!("   watches {}", folder.text));
    }
    for given in &unit.gives {
        given_lines(lines, given);
    }
}

fn running(runs: &Running) -> String {
    match runs {
        Running::OnHost => "runs on the host".to_string(),
        Running::OwnContainer => "runs in its own container".to_string(),
        Running::InsideUnit(unit) => format!("runs inside the container of `{unit}`"),
        Running::OneOffContainer => "runs in a container made for the run".to_string(),
    }
}

fn given_lines(lines: &mut Vec<String>, given: &GivenLine) {
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
        Seen::Value(value) => value.text.clone(),
        Seen::Unreachable(why) => format!("not reachable: {why}"),
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
