//! `tay run ACTION [NAME=VALUE...]`: runs a unit that has `action: true`,
//! with its arguments, after bringing up what it needs, on an environment
//! that is up or down, which it leaves as it found it. The daemon makes
//! it, and is started if none runs. The arguments are checked as the plan
//! is made, before anything runs. It prints what the action writes as it
//! writes it, then each value the action produced, by name, a secret one
//! never; and it exits as the action did. Nothing of what the action gave
//! is kept.

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, Words};
use crate::cli::output::change::Printer;
use crate::cli::stack;
use crate::control::client::Client;
use crate::control::protocol::RunRequest;
use crate::disk::{Environment, Layout};
use crate::plan::ActionRun;

use super::plan::{no_stack, options, planned};
use super::{block_on, client_problem, disk_problem, forget_what_is_gone, machine, problem};

pub const TAKES: Takes = Takes {
    command: "tay run",
    choices: false,
    json: true,
    follows: false,
    wipes: false,
    words: Words::Any,
};

const COMMAND: &str = "tay run";

pub fn run(args: Args) -> Exit {
    let mut words = args.words.into_iter();
    let Some(action) = words.next() else {
        return problem(COMMAND, Exit::Errors, "name the action to run");
    };
    let run = ActionRun {
        action,
        args: words.collect(),
    };
    let root = match stack::root(args.root.as_deref()) {
        Ok(root) => root,
        Err(message) => return no_stack(COMMAND, &message, args.json),
    };
    let mut options = match options(&root, None, &[], &[]) {
        Ok(options) => options,
        Err(error) => return disk_problem(COMMAND, &error),
    };
    // An action or an argument that does not fit the stack stops it here,
    // with nothing run.
    options.action = Some(run.clone());
    if let Err(exit) = planned(COMMAND, &root, &options, args.json, "nothing was run") {
        return exit;
    }
    let machine = match machine(COMMAND) {
        Ok(machine) => machine,
        Err(exit) => return exit,
    };
    let request = RunRequest {
        run,
        home: options.home,
    };
    let ended = block_on(COMMAND, async {
        // An environment never brought up is down, and is given its id as
        // `tay up` gives it, since what the action needs is kept by it.
        let established = match forget_what_is_gone(&machine).await {
            Ok(_) => Environment::establish(&Layout::of(&root), &machine),
            Err(error) => Err(error),
        };
        let environment = match established {
            Ok(environment) => environment,
            Err(error) => return disk_problem(COMMAND, &error),
        };
        let client = match Client::ensure(&environment).await {
            Ok(client) => client,
            Err(error) => return client_problem(COMMAND, &error),
        };
        let mut printer = Printer::beside_output(COMMAND, args.json);
        if let Err(error) = client
            .run(&request, &mut |event| printer.event(event))
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
