//! `tay set [PLACE=VALUE...]`: changes what the environment remembers being
//! asked for, in the places `--set` takes and in `profile`, which it
//! remembers as `--profile` does, and touches no unit until the next
//! `tay up`; with nothing after it, lists what is remembered. Each is
//! checked as `tay up --set` checks it, by planning with it, and a stack
//! that does not plan with it remembers nothing of it. What is remembered is
//! listed afterwards, with the value of every secret setting hidden.

use std::path::Path;

use serde_json::json;

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, Words};
use crate::cli::output::asked;
use crate::cli::stack;
use crate::control::client::Client;
use crate::control::protocol::Remember;
use crate::definition::read::THIS_TAY;
use crate::disk::{Asked, Environment, Identity, Layout};
use crate::print::out;

use super::plan::{no_stack, options, planned, print_json};
use super::{block_on, client_problem, disk_problem, machine};

pub const TAKES: Takes = Takes {
    command: "tay set",
    choices: false,
    json: true,
    follows: false,
    wipes: false,
    words: Words::Any,
};

/// The schema of `--json`. A change to any key or its meaning is a new
/// number.
const SCHEMA: u32 = 1;

const COMMAND: &str = "tay set";

pub fn run(args: Args) -> Exit {
    let root = match stack::root(args.root.as_deref()) {
        Ok(root) => root,
        Err(message) => return no_stack(COMMAND, &message, args.json),
    };
    if args.words.is_empty() {
        return list(COMMAND, &root, args.json);
    }
    // `profile=NAME` names the profile, as `--profile` does; every other
    // word is a `--set`.
    let mut profile = None;
    let mut sets = Vec::new();
    for word in args.words {
        match word.split_once('=') {
            Some((Asked::PROFILE, name)) => profile = Some(name.to_string()),
            _ => sets.push(word),
        }
    }
    let options = match options(&root, profile.clone(), &sets, &[]) {
        Ok(options) => options,
        Err(error) => return disk_problem(COMMAND, &error),
    };
    if let Err(exit) = planned(
        COMMAND,
        &root,
        &options,
        args.json,
        "nothing was remembered",
    ) {
        return exit;
    }
    remember(COMMAND, &root, &Remember::Set { profile, sets }, args.json)
}

/// Makes `change` to what the environment at `root` remembers, as one
/// change, and lists what it remembers then. An environment never brought
/// up, or a copy of another, has no daemon, so nothing else can be changing
/// it.
pub fn remember(command: &str, root: &Path, change: &Remember, json: bool) -> Exit {
    let machine = match machine(command) {
        Ok(machine) => machine,
        Err(exit) => return exit,
    };
    let layout = Layout::of(root);
    let made = match Environment::identify(&layout, &machine) {
        Ok(Identity::Own(environment)) => {
            match block_on(command, Client::remember(&environment, change)) {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(client_problem(command, &error)),
                Err(exit) => Err(exit),
            }
        }
        Ok(Identity::New | Identity::CopyOf { .. }) => change
            .apply(&layout)
            .map_err(|error| disk_problem(command, &error)),
        Err(error) => Err(disk_problem(command, &error)),
    };
    match made {
        Ok(()) => list(command, root, json),
        Err(exit) => exit,
    }
}

/// Lists what the environment at `root` remembers being asked for.
fn list(command: &str, root: &Path, json: bool) -> Exit {
    let asked = match Asked::read(&Layout::of(root)) {
        Ok(remembered) => asked::shown(remembered, asked::stack(root).as_deref()),
        Err(error) => return disk_problem(command, &error),
    };
    if json {
        print_json(&json!({
            "schema": SCHEMA,
            "tay": THIS_TAY,
            "asked": {
                "profile": asked.profile,
                "sets": asked.sets,
                "units": asked.units,
            },
        }));
        return Exit::Done;
    }
    if asked.is_empty() {
        out("nothing is remembered\n");
        return Exit::Done;
    }
    let mut text = String::new();
    if let Some(profile) = &asked.profile {
        text.push_str(&format!("profile  {profile}\n"));
    }
    if !asked.units.is_empty() {
        text.push_str(&format!("units    {}\n", asked.units.join(" ")));
    }
    for set in &asked.sets {
        text.push_str(&format!("set      {set}\n"));
    }
    out(&text);
    Exit::Done
}
