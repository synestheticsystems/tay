//! `tay down`: stops everything the environment runs, checks that nothing
//! is left, and says so; then the daemon exits. With no daemon and an
//! environment meant to be up, a daemon is started to find what runs.

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, UnitsNamed};
use crate::cli::output::change::Printer;
use crate::cli::stack;
use crate::control::client::Client;
use crate::control::protocol::Event;
use crate::disk::{Environment, Identity, Intent, Layout};

use super::plan::no_stack;
use super::{block_on, client_problem, disk_problem, machine, problem};

pub const TAKES: Takes = Takes {
    command: "tay down",
    choices: false,
    json: true,
    units: UnitsNamed::None,
};

const COMMAND: &str = "tay down";

pub fn run(args: Args) -> Exit {
    let root = match stack::root(args.root.as_deref()) {
        Ok(root) => root,
        Err(message) => return no_stack(COMMAND, &message, args.json),
    };
    let machine = match machine(COMMAND) {
        Ok(machine) => machine,
        Err(exit) => return exit,
    };
    let layout = Layout::of(&root);
    let environment = match Environment::identify(&layout, &machine) {
        Ok(Identity::Own(environment)) => environment,
        Ok(Identity::New | Identity::CopyOf { .. }) => return nothing_runs(args.json),
        Err(error) => return disk_problem(COMMAND, &error),
    };
    let ended = block_on(COMMAND, async {
        let found = match Client::find(&environment).await {
            Ok(found) => found,
            Err(error) => return client_problem(COMMAND, &error),
        };
        let client = match (found, Intent::read(&environment.layout)) {
            (Some(client), _) => client,
            (None, Ok(Intent::Down)) => return nothing_runs(args.json),
            (None, Ok(Intent::Up)) => match Client::ensure(&environment).await {
                Ok(client) => client,
                Err(error) => return client_problem(COMMAND, &error),
            },
            (None, Err(error)) => return disk_problem(COMMAND, &error),
        };
        let mut printer = Printer::new(COMMAND, args.json);
        if let Err(error) = client.down(&mut |event| printer.event(event)).await {
            return client_problem(COMMAND, &error);
        }
        printer.exit().unwrap_or_else(|| {
            problem(
                COMMAND,
                Exit::Unreachable,
                "the daemon ended without saying what it stopped",
            )
        })
    });
    match ended {
        Ok(exit) | Err(exit) => exit,
    }
}

/// Nothing of the environment runs: it is down, or was never brought up.
fn nothing_runs(json: bool) -> Exit {
    Printer::new(COMMAND, json).event(Event::Down { left: Vec::new() });
    Exit::Done
}
