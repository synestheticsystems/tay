//! The plan as a person reads it.

use std::fmt::Write;

use crate::plan::{Because, GivenLine, Plan, PlannedUnit, Readiness, Running, Seen, Way};

pub fn plan(plan: &Plan) -> String {
    let mut out = String::new();
    let profile = match (&plan.profile, plan.production) {
        (Some(profile), true) => format!("profile `{profile}` (production)"),
        (Some(profile), false) => format!("profile `{profile}`"),
        (None, _) => "no profile".to_string(),
    };
    let _ = writeln!(
        out,
        "Plan for stack `{}`, {profile}, in {}",
        plan.stack, plan.root
    );
    match plan.units.len() {
        0 => {
            let _ = writeln!(out, "Nothing comes up: `up` names no unit.");
        }
        1 => {
            let _ = writeln!(out, "1 unit comes up.");
        }
        n => {
            let _ = writeln!(out, "{n} units come up, in an order they could start in.");
        }
    }
    for (i, unit) in plan.units.iter().enumerate() {
        out.push('\n');
        unit_block(&mut out, i + 1, unit);
    }
    out
}

fn unit_block(out: &mut String, number: usize, unit: &PlannedUnit) {
    let way = match &unit.way {
        Way::Group => "a group".to_string(),
        Way::Module {
            module,
            alternative,
            runs,
        } => {
            let alternative = alternative
                .as_ref()
                .map(|alt| format!(" (alternative `{alt}`)"))
                .unwrap_or_default();
            format!("uses `{module}`{alternative}, {}", running(runs))
        }
    };
    let _ = writeln!(out, "{number}. {}: {way}", unit.name);
    if let Some(doc) = &unit.doc {
        let _ = writeln!(out, "   {doc}");
    }
    for need in &unit.needs {
        let _ = match &need.because {
            Because::Written => writeln!(out, "   needs {}", need.unit),
            Because::Uses(value) => writeln!(
                out,
                "   needs {}, because it uses {value}, which {} produces by running",
                need.unit, need.unit
            ),
        };
    }
    for after in &unit.after {
        let comes_up = if after.comes_up {
            ""
        } else {
            ", which does not come up"
        };
        let _ = writeln!(out, "   waits for {} (after){comes_up}", after.unit);
    }
    match &unit.ready {
        Readiness::Group => {}
        Readiness::Default(how) => {
            let _ = writeln!(out, "   ready: {how}");
        }
        Readiness::Check {
            check,
            target,
            within,
        } => {
            let target: Vec<&str> = target.iter().map(|t| t.text.as_str()).collect();
            let _ = writeln!(
                out,
                "   ready: {check} check of {}, within {}",
                target.join(" "),
                within.text
            );
        }
    }
    if let Some(restart) = &unit.restart {
        let attempts = restart
            .attempts
            .as_ref()
            .map(|a| format!(", up to {} attempts", a.text))
            .unwrap_or_default();
        let _ = match restart.on.text.as_str() {
            "never" => writeln!(out, "   restart: never"),
            on => writeln!(out, "   restart: on {on}{attempts}"),
        };
    }
    for folder in &unit.watch {
        let _ = writeln!(out, "   watches {}", folder.text);
    }
    for given in &unit.gives {
        given_line(out, given);
    }
}

fn running(runs: &Running) -> String {
    match runs {
        Running::OnHost => "runs on the host".to_string(),
        Running::OwnContainer => "runs in its own container".to_string(),
        Running::InsideUnit(unit) => format!("runs inside the container of `{unit}`"),
        Running::OneOffContainer => "runs in a container made for the run".to_string(),
        Running::ByTay => "tay does it itself".to_string(),
    }
}

fn given_line(out: &mut String, given: &GivenLine) {
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
        let _ = writeln!(out, "   gives {} = {host}{notes}", given.name);
    } else {
        let _ = writeln!(out, "   gives {}{notes}", given.name);
        let _ = writeln!(out, "     from the host: {host}");
        let _ = writeln!(out, "     from a container: {containers}");
    }
}
