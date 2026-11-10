//! `tay restart UNIT...`: stops each unit named and starts it again, with a
//! new first attempt, bringing up what it needs as `tay up` would; units
//! that need it are left as they run. The daemon makes it, from the plan it
//! holds, and is started if none runs and the environment is meant to be
//! up; on one that is down there is nothing to restart. It prints each
//! change of state, and returns when the units named are ready or one can
//! no longer become ready.

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, Words};
use crate::cli::output::change::Printer;
use crate::cli::stack;
use crate::control::client::ClientError;
use crate::control::protocol::{Refused, Restart};
use crate::disk::{Environment, Identity, Layout};
use crate::syntax::names;

use super::plan::no_stack;
use super::{block_on, client_problem, daemon_for_up, disk_problem, machine, problem};

pub const TAKES: Takes = Takes {
    command: "tay restart",
    choices: false,
    json: true,
    follows: false,
    wipes: false,
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
            let refused = ClientError::Refused(Refused::environment_down());
            return client_problem(COMMAND, &refused);
        }
        Err(error) => return disk_problem(COMMAND, &error),
    };
    let restart = Restart { units: args.words };
    let ended = block_on(COMMAND, async {
        let client = match daemon_for_up(COMMAND, &environment).await {
            Ok(client) => client,
            Err(exit) => return exit,
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
