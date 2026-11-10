//! `tay status [UNIT]`: each unit's state, from a check. The daemon makes
//! the checks; with none running, an environment meant to be up, or with
//! anything of it left, has one started to find its units, and any other
//! has every unit stopped.

use std::path::Path;

use serde_json::json;

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, Words};
use crate::cli::output::asked;
use crate::cli::stack;
use crate::control::client::{Client, ClientError};
use crate::definition::read::THIS_TAY;
use crate::disk::{Asked, Environment, Identity, Intent, Layout};
use crate::engine::State;
use crate::module;
use crate::print::out;

use super::plan::{no_stack, options, planned, print_json};
use super::{block_on, client_problem, disk_problem, machine, problem};

pub const TAKES: Takes = Takes {
    command: "tay status",
    choices: false,
    json: true,
    follows: false,
    words: Words::AtMostOneUnit,
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
    /// What the environment remembers being asked for, with the value of
    /// every secret setting a `--set` gives hidden.
    asked: Asked,
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
    report.asked = match Asked::read(&Layout::of(&root)) {
        Ok(asked) => asked::shown(asked, &root),
        Err(error) => return disk_problem(COMMAND, &error),
    };
    if let [unit] = args.words.as_slice() {
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
            None => {
                let left = module::found_left(environment)
                    .await
                    .map_err(|failure| Unreached::Left(failure.to_string()))?;
                if intent == Intent::Down && left.is_empty() {
                    return Ok(None);
                }
                Client::ensure(environment).await?
            }
        };
        Ok(Some(client.status().await?))
    })?;
    match status {
        Ok(Some(status)) => Ok(Report {
            root: root.display().to_string(),
            id,
            standing: intent.word(),
            asked: Asked::default(),
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
        Err(Unreached::Client(error)) => Err(client_problem(COMMAND, &error)),
        Err(Unreached::Left(why)) => Err(problem(COMMAND, Exit::Unreachable, &why)),
    }
}

/// Why no daemon could say how the units stand.
enum Unreached {
    Client(ClientError),
    /// What is left of the environment's units cannot be told.
    Left(String),
}

impl From<ClientError> for Unreached {
    fn from(error: ClientError) -> Unreached {
        Unreached::Client(error)
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
    let options = options(root, None, &[], &[]).map_err(|error| disk_problem(COMMAND, &error))?;
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
        asked: Asked::default(),
        units,
    })
}

/// What was asked, as the plain `tay up` it now stands for.
fn remembered(asked: &Asked) -> String {
    let mut words = vec!["tay up".to_string()];
    words.extend(asked.units.iter().cloned());
    if let Some(profile) = &asked.profile {
        words.push(format!("--profile {profile}"));
    }
    words.extend(asked.sets.iter().map(|set| format!("--set {set}")));
    words.join(" ")
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
                "asked": {
                    "profile": report.asked.profile,
                    "sets": report.asked.sets,
                    "units": report.asked.units,
                },
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
    if !report.asked.is_empty() {
        text.push_str(&format!("  remembered: {}\n", remembered(&report.asked)));
    }
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
