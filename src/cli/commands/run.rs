//! `tay run ACTION [NAME=VALUE...]`: runs a unit that has `action: true`,
//! with its arguments, after bringing up what it needs. The arguments are
//! checked as the plan is made, before anything runs. It prints what the
//! action writes as it writes it, then each value the action produced, by
//! name, a secret one never; and it exits as the action did. Nothing of
//! what the action gave is kept.

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, Words};
use crate::cli::output::change::Printer;
use crate::cli::stack;
use crate::control::client::ClientError;
use crate::control::protocol::{Refused, RunRequest};
use crate::disk::{Environment, Identity, Layout};
use crate::plan::ActionRun;

use super::plan::{no_stack, options, planned};
use super::{block_on, client_problem, daemon_for_up, disk_problem, machine, problem};

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
    let environment = match Environment::identify(&Layout::of(&root), &machine) {
        Ok(Identity::Own(environment)) => environment,
        Ok(Identity::New | Identity::CopyOf { .. }) => {
            let refused = ClientError::Refused(Refused::environment_down());
            return client_problem(COMMAND, &refused);
        }
        Err(error) => return disk_problem(COMMAND, &error),
    };
    let request = RunRequest {
        run,
        home: options.home,
    };
    let ended = block_on(COMMAND, async {
        let client = match daemon_for_up(COMMAND, &environment).await {
            Ok(client) => client,
            Err(exit) => return exit,
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
