//! `tay status [UNIT]`: each unit's state, from a check. The daemon makes
//! the checks; with none running, an environment meant to be up has one
//! started to find its units, and any other has every unit stopped.

use std::path::Path;

use serde_json::json;

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, UnitsNamed};
use crate::cli::stack;
use crate::control::client::Client;
use crate::definition::read::THIS_TAY;
use crate::disk::{Environment, Identity, Intent, Layout};
use crate::engine::State;
use crate::plan::Options;
use crate::print::out;

use super::plan::{no_stack, options, planned, print_json};
use super::{block_on, client_problem, disk_problem, machine, problem};

pub const TAKES: Takes = Takes {
    command: "tay status",
    choices: false,
    json: true,
    units: UnitsNamed::AtMostOne,
};

/// The schema of `--json`. A change to any key or its meaning is a new
/// number.
const SCHEMA: u32 = 1;

const COMMAND: &str = "tay status";

/// An environment as `tay status` reports it.
struct Report {
    root: String,
    id: Option<String>,
    /// `up`, `down`, or `never brought up`.
    standing: &'static str,
    units: Vec<Line>,
}

/// One unit as `tay status` reports it.
struct Line {
    unit: String,
    state: State,
    detail: String,
    /// When it took its state, in seconds since 1970; `None` when no daemon
    /// recorded it.
    since: Option<u64>,
}

pub fn run(args: Args) -> Exit {
    let root = match stack::root(args.root.as_deref()) {
        Ok(root) => root,
        Err(message) => return no_stack(COMMAND, &message, args.json),
    };
    let machine = match machine(COMMAND) {
        Ok(machine) => machine,
        Err(exit) => return exit,
    };
    let report = match Environment::identify(&Layout::of(&root), &machine) {
        Ok(Identity::Own(environment)) => from_daemon(&environment, &args),
        Ok(Identity::New) => stopped(&root, None, "never brought up", "never brought up", &args),
        Ok(Identity::CopyOf { original }) => {
            let detail = format!(
                "never brought up: this folder is a copy of `{}`",
                original.display()
            );
            stopped(&root, None, "never brought up", &detail, &args)
        }
        Err(error) => Err(disk_problem(COMMAND, &error)),
    };
    let mut report = match report {
        Ok(report) => report,
        Err(exit) => return exit,
    };
    if let [unit] = args.units.as_slice() {
        report.units.retain(|line| line.unit == *unit);
        if report.units.is_empty() {
            return problem(
                COMMAND,
                Exit::Errors,
                &format!("there is no unit `{unit}` in this environment's plan"),
            );
        }
    }
    print(&report, args.json);
    match report.units.iter().any(|line| line.state == State::Failed) {
        true => Exit::Failed,
        false => Exit::Done,
    }
}

/// The report of an environment with an id of its own: from its daemon,
/// started if the environment is meant to be up and none runs.
fn from_daemon(environment: &Environment, args: &Args) -> Result<Report, Exit> {
    let intent =
        Intent::read(&environment.layout).map_err(|error| disk_problem(COMMAND, &error))?;
    let root = environment.layout.root();
    let id = Some(environment.id.to_string());
    let status = block_on(COMMAND, async {
        let client = match Client::find(environment).await? {
            Some(client) => client,
            None if intent == Intent::Down => return Ok(None),
            None => Client::ensure(environment).await?,
        };
        client.status().await.map(Some)
    })?;
    match status {
        Ok(Some(status)) => Ok(Report {
            root: root.display().to_string(),
            id,
            standing: intent.word(),
            units: status
                .units
                .into_iter()
                .map(|status| Line {
                    unit: status.unit,
                    state: status.state,
                    detail: status.detail,
                    since: Some(status.since),
                })
                .collect(),
        }),
        // No daemon runs and the environment is down: nothing runs.
        Ok(None) => stopped(root, id, "down", "the environment is down", args),
        Err(error) => Err(client_problem(COMMAND, &error)),
    }
}

/// Every unit `tay plan` would list, stopped.
fn stopped(
    root: &Path,
    id: Option<String>,
    standing: &'static str,
    detail: &str,
    args: &Args,
) -> Result<Report, Exit> {
    // The unit `tay status` names is the one it reports, not one to plan.
    let options = Options {
        units: Vec::new(),
        ..options(args)
    };
    let plan = planned(COMMAND, root, &options, args.json, "there is no plan")?;
    let units = plan
        .units
        .into_iter()
        .map(|unit| Line {
            unit: unit.name,
            state: State::Stopped,
            detail: detail.to_string(),
            since: None,
        })
        .collect();
    Ok(Report {
        root: root.display().to_string(),
        id,
        standing,
        units,
    })
}

fn print(report: &Report, json: bool) {
    if json {
        print_json(&json!({
            "schema": SCHEMA,
            "tay": THIS_TAY,
            "environment": {
                "root": report.root,
                "id": report.id,
                "standing": report.standing,
            },
            "units": report.units.iter().map(|line| json!({
                "unit": line.unit,
                "state": line.state.word(),
                "detail": line.detail,
                "since": line.since,
            })).collect::<Vec<_>>(),
        }));
        return;
    }
    let width = report
        .units
        .iter()
        .map(|line| line.unit.len())
        .max()
        .unwrap_or(0);
    let mut text = format!("{}: {}\n", report.root, report.standing);
    for line in &report.units {
        text.push_str(&format!(
            "  {:<width$}  {:<8}  {}\n",
            line.unit,
            line.state.word(),
            line.detail
        ));
    }
    out(&text);
}
