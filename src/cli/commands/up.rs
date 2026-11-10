//! `tay up [UNIT...]`: plans exactly as `tay plan` does, starts the
//! environment's daemon if none runs, hands it what to bring up, and prints
//! each change of state until everything asked for is ready or one of it
//! can no longer become ready.

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, UnitsNamed};
use crate::cli::output::change::Printer;
use crate::cli::stack;
use crate::control::client::Client;
use crate::control::protocol::UpRequest;
use crate::disk::{Environment, Layout};

use super::plan::{no_stack, options, planned};
use super::{block_on, client_problem, disk_problem, machine, problem};

pub const TAKES: Takes = Takes {
    command: "tay up",
    choices: true,
    json: true,
    units: UnitsNamed::Any,
};

const COMMAND: &str = "tay up";

pub fn run(args: Args) -> Exit {
    let root = match stack::root(args.root.as_deref()) {
        Ok(root) => root,
        Err(message) => return no_stack(COMMAND, &message, args.json),
    };
    let options = match options(&root, args.profile.clone(), &args.sets, &args.units) {
        Ok(options) => options,
        Err(error) => return disk_problem(COMMAND, &error),
    };
    // Errors in the definitions stop it here, with nothing started.
    if let Err(exit) = planned(COMMAND, &root, &options, args.json, "nothing was started") {
        return exit;
    }
    let machine = match machine(COMMAND) {
        Ok(machine) => machine,
        Err(exit) => return exit,
    };
    let environment = match Environment::establish(&Layout::of(&root), &machine) {
        Ok(environment) => environment,
        Err(error) => return disk_problem(COMMAND, &error),
    };
    let request = UpRequest {
        asked: options.asked,
        home: options.home,
    };
    let ended = block_on(COMMAND, async {
        let client = match Client::ensure(&environment).await {
            Ok(client) => client,
            Err(error) => return client_problem(COMMAND, &error),
        };
        let mut printer = Printer::new(COMMAND, args.json);
        if let Err(error) = client.up(&request, &mut |event| printer.event(event)).await {
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
