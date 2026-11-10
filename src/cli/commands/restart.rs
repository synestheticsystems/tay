//! `tay restart UNIT...`: stops each unit named and starts it again, with a
//! new first attempt, bringing up what it needs as `tay up` would; units
//! that need it are left as they run. The daemon makes it, from the plan it
//! holds, and is started if none runs; it prints each change of state, and
//! returns when the units named are ready or one can no longer become
//! ready.

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, Words};
use crate::cli::output::change::Printer;
use crate::cli::stack;
use crate::control::client::Client;
use crate::control::protocol::Restart;
use crate::disk::{Environment, Identity, Layout};
use crate::syntax::names;

use super::plan::no_stack;
use super::{block_on, client_problem, disk_problem, machine, problem};

pub const TAKES: Takes = Takes {
    command: "tay restart",
    choices: false,
    json: true,
    follows: false,
    words: Words::Any,
};

const COMMAND: &str = "tay restart";

pub fn run(args: Args) -> Exit {
    if args.words.is_empty() {
        return problem(COMMAND, Exit::Errors, "name each unit to restart");
    }
    if let Some(why) = args.words.iter().find_map(|unit| names::name_problem(unit)) {
        return problem(COMMAND, Exit::Errors, &why);
    }
    let root = match stack::root(args.root.as_deref()) {
        Ok(root) => root,
        Err(message) => return no_stack(COMMAND, &message, args.json),
    };
    let machine = match machine(COMMAND) {
        Ok(machine) => machine,
        Err(exit) => return exit,
    };
    let environment = match Environment::identify(&Layout::of(&root), &machine) {
        Ok(Identity::Own(environment)) => environment,
        Ok(Identity::New | Identity::CopyOf { .. }) => {
            return problem(
                COMMAND,
                Exit::Errors,
                "this environment has never been brought up: `tay up` brings it up",
            );
        }
        Err(error) => return disk_problem(COMMAND, &error),
    };
    let restart = Restart { units: args.words };
    let ended = block_on(COMMAND, async {
        let client = match Client::ensure(&environment).await {
            Ok(client) => client,
            Err(error) => return client_problem(COMMAND, &error),
        };
        let mut printer = Printer::new(COMMAND, args.json);
        if let Err(error) = client
            .restart(&restart, &mut |event| printer.event(event))
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
