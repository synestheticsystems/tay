//! `tay wipe UNIT... --yes` and `tay wipe --all --yes`: removes what the
//! units named keep, so that each starts from nothing, or takes the
//! environment down and removes everything it keeps. The daemon makes it,
//! as one change, and is started if none runs. Without `--yes` nothing
//! changes, and it says what it would remove.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, Words};
use crate::cli::output::change::Printer;
use crate::cli::stack;
use crate::control::client::Client;
use crate::control::protocol::{Event, FOLDER_WIPED, LISTING_WIPED, WipeRequest};
use crate::definition::read::THIS_TAY;
use crate::disk::{Environment, Identity, Layout, Lock};
use crate::module::{self, UnitPlace};
use crate::plan::Plan;
use crate::print::out;
use crate::syntax::names;

use super::plan::{no_stack, options, planned, print_json};
use super::{block_on, client_problem, disk_problem, machine, problem};

pub const TAKES: Takes = Takes {
    command: "tay wipe",
    choices: false,
    json: true,
    follows: false,
    wipes: true,
    words: Words::Any,
};

/// The schema of `--json` without `--yes`. A change to any key or its
/// meaning is a new number.
const SCHEMA: u32 = 1;

const COMMAND: &str = "tay wipe";

pub fn run(args: Args) -> Exit {
    match (args.all, args.words.is_empty()) {
        (true, false) => {
            return problem(
                COMMAND,
                Exit::Errors,
                "`--all` wipes everything the environment keeps: name no unit with it",
            );
        }
        (false, true) => {
            return problem(
                COMMAND,
                Exit::Errors,
                "name each unit to wipe, or `--all` for everything the environment keeps",
            );
        }
        _ => {}
    }
    if let Some(why) = args.words.iter().find_map(|unit| names::name_problem(unit)) {
        return problem(COMMAND, Exit::Errors, &why);
    }
    let root = match stack::root(args.root.as_deref()) {
        Ok(root) => root,
        Err(message) => return no_stack(COMMAND, &message, args.json),
    };
    let options = match options(&root, None, &[], &[]) {
        Ok(options) => options,
        Err(error) => return disk_problem(COMMAND, &error),
    };
    let plan = match planned(COMMAND, &root, &options, args.json, "nothing was removed") {
        Ok(plan) => plan,
        Err(exit) => return exit,
    };
    for unit in &args.words {
        match plan.units.iter().find(|planned| planned.name == *unit) {
            None => {
                let why = format!("there is no unit `{unit}` in this environment's plan");
                return problem(COMMAND, Exit::Errors, &why);
            }
            Some(planned) if planned.mechanism.is_none() => {
                let why = format!("`{unit}` is a group, which keeps nothing of its own");
                return problem(COMMAND, Exit::Errors, &why);
            }
            Some(_) => {}
        }
    }
    let machine = match machine(COMMAND) {
        Ok(machine) => machine,
        Err(exit) => return exit,
    };
    let layout = Layout::of(&root);
    let environment = match Environment::identify(&layout, &machine) {
        Ok(Identity::Own(environment)) => environment,
        // Never brought up, or a copy of another, it keeps nothing of its
        // own but its folder.
        Ok(Identity::New | Identity::CopyOf { .. }) => {
            return never_brought_up(&layout, &args);
        }
        Err(error) => return disk_problem(COMMAND, &error),
    };
    if !args.yes {
        return match kept(&environment, &plan, &args) {
            Ok(kept) => say_kept(&kept, args.json),
            Err(exit) => exit,
        };
    }
    let request = match args.all {
        true => WipeRequest::All,
        false => WipeRequest::Units {
            units: args.words,
            home: options.home,
        },
    };
    let ended = block_on(COMMAND, async {
        let client = match Client::ensure(&environment).await {
            Ok(client) => client,
            Err(error) => return client_problem(COMMAND, &error),
        };
        let mut printer = Printer::new(COMMAND, args.json);
        if let Err(error) = client
            .wipe(&request, &mut |event| printer.event(event))
            .await
        {
            return client_problem(COMMAND, &error);
        }
        printer.exit().unwrap_or_else(|| {
            problem(
                COMMAND,
                Exit::Unreachable,
                "the daemon ended the change without saying how",
            )
        })
    });
    match ended {
        Ok(exit) | Err(exit) => exit,
    }
}

/// What is kept, by unit, or with no unit what the environment keeps.
type Kept = Vec<(Option<String>, Vec<String>)>;

/// What `--yes` would remove: what each unit named keeps, as its module
/// answers, or everything the environment keeps.
fn kept(environment: &Environment, plan: &Plan, args: &Args) -> Result<Kept, Exit> {
    if args.all {
        let outside = block_on(COMMAND, module::kept_by_environment(environment))?
            .map_err(|failure| problem(COMMAND, Exit::Unreachable, &failure.to_string()))?;
        let everything = outside
            .into_iter()
            .chain([FOLDER_WIPED, LISTING_WIPED].map(str::to_string));
        return Ok(vec![(None, everything.collect())]);
    }
    let environment = Arc::new(environment.clone());
    let kept = (args.words.iter())
        .filter_map(|unit| plan.units.iter().find(|planned| planned.name == *unit))
        .filter_map(|planned| {
            let place = UnitPlace {
                unit: planned.name.clone(),
                environment: Arc::clone(&environment),
            };
            let kept = planned.mechanism.as_ref()?.settings.kept(&place);
            Some((Some(planned.name.clone()), kept.what))
        })
        .collect();
    Ok(kept)
}

/// Says what `--yes` would remove, and that nothing was.
fn say_kept(kept: &Kept, json: bool) -> Exit {
    if json {
        let kept: Vec<Value> = kept
            .iter()
            .map(|(unit, what)| json!({ "unit": unit, "kept": what }))
            .collect();
        print_json(&json!({
            "schema": SCHEMA,
            "tay": THIS_TAY,
            "kept": kept,
            "removed": false,
        }));
        return Exit::Done;
    }
    let mut text = String::new();
    for (unit, what) in kept {
        let what = match what.is_empty() {
            true => "nothing".to_string(),
            false => what.join("; "),
        };
        match unit {
            Some(unit) => text.push_str(&format!("{COMMAND}: `{unit}` keeps {what}\n")),
            None => text.push_str(&format!(
                "{COMMAND}: --all takes the environment down and removes {what}\n"
            )),
        }
    }
    text.push_str(&format!(
        "{COMMAND}: nothing was removed; `--yes` removes it.\n"
    ));
    out(&text);
    Exit::Done
}

/// An environment never brought up, or a copy of another, keeps nothing
/// of its own but its folder, which `--all` removes, under the daemon's
/// lock so that no daemon begins meanwhile.
fn never_brought_up(layout: &Layout, args: &Args) -> Exit {
    if !args.all {
        let kept: Kept = (args.words.iter())
            .map(|unit| (Some(unit.clone()), Vec::new()))
            .collect();
        return say_kept(&kept, args.json);
    }
    let removed = vec![FOLDER_WIPED.to_string()];
    if !args.yes {
        return say_kept(&vec![(None, removed)], args.json);
    }
    if let Err(exit) = remove_folder(layout) {
        return exit;
    }
    let mut printer = Printer::new(COMMAND, args.json);
    printer.event(Event::Removed {
        unit: None,
        removed,
    });
    printer.event(Event::Wiped);
    Exit::Done
}

/// Removes `.tay/` but for `choices.yaml`, under the daemon's lock.
fn remove_folder(layout: &Layout) -> Result<(), Exit> {
    let lock = Lock::take(&layout.daemon_lock()).map_err(|error| disk_problem(COMMAND, &error))?;
    let removed = layout.remove_kept();
    drop(lock);
    removed.map_err(|error| disk_problem(COMMAND, &error))
}
