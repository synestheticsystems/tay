//! `tay serve`: is the environment's daemon in the foreground, for a
//! service manager to start, restart and stop. It plans as `tay up` does,
//! takes the place of a daemon that runs for the environment, which leaves
//! the stack running, brings up what the environment remembers being asked
//! for, and prints each change of state as `tay up` does until it is told
//! to stop, or `tay down` takes the environment down.

use std::sync::{Arc, Mutex};

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, Words};
use crate::cli::output::change::Printer;
use crate::cli::stack;
use crate::control::client::Client;
use crate::control::daemon::{self, Journal, Served};
use crate::control::protocol::Event;
use crate::disk::{Environment, Layout};

use super::plan::{no_stack, options, planned};
use super::{block_on, client_problem, disk_problem, forget_what_is_gone, machine, problem};

pub const TAKES: Takes = Takes {
    command: "tay serve",
    choices: false,
    json: false,
    follows: false,
    wipes: false,
    words: Words::None,
};

const COMMAND: &str = "tay serve";

pub fn run(args: Args) -> Exit {
    let ended = block_on(COMMAND, serve(args));
    match ended {
        Ok(exit) | Err(exit) => exit,
    }
}

async fn serve(args: Args) -> Exit {
    // From its first moment, so that no signal it is sent ends it before
    // it heeds them.
    let signals = match daemon::Signals::new() {
        Ok(signals) => signals,
        Err(why) => return problem(COMMAND, Exit::Unreachable, &why.to_string()),
    };
    let root = match stack::root(args.root.as_deref()) {
        Ok(root) => root,
        Err(message) => return no_stack(COMMAND, &message, false),
    };
    let options = match options(&root, None, &[], &[]) {
        Ok(options) => options,
        Err(error) => return disk_problem(COMMAND, &error),
    };
    // Errors in the definitions stop it here, with nothing started, as
    // they stop `tay up`.
    let plan = match planned(COMMAND, &root, &options, false, "nothing was started") {
        Ok(plan) => plan,
        Err(exit) => return exit,
    };
    let machine = match machine(COMMAND) {
        Ok(machine) => machine,
        Err(exit) => return exit,
    };
    let printer = Arc::new(Mutex::new(Printer::new(COMMAND, false)));
    let print = {
        let printer = Arc::clone(&printer);
        move |event| {
            printer
                .lock()
                .expect("the printer is not poisoned")
                .event(event);
        }
    };
    // The units it finds again are said before its bring-up names them,
    // and line up with them.
    print(Event::Units {
        units: plan.units.iter().map(|unit| unit.name.clone()).collect(),
    });
    // The place is taken before what takes time, so that a command started
    // meanwhile, as an install script's `tay up`, waits for this daemon
    // rather than start one of its own that this one would replace.
    let environment = match Environment::establish(&Layout::of(&root), &machine) {
        Ok(environment) => environment,
        Err(error) => return disk_problem(COMMAND, &error),
    };
    let place = match Client::take_daemons_place(&environment).await {
        Ok(place) => place,
        Err(error) => return client_problem(COMMAND, &error),
    };
    // One that stays because whether anything of it is left cannot be told
    // is said so by `tay ls`.
    if let Err(error) = forget_what_is_gone(&machine).await {
        return disk_problem(COMMAND, &error);
    }
    let journal: Journal = Arc::new(print.clone());
    match daemon::serve(environment.layout, place, signals, options.home, journal).await {
        Ok(Served::Down(went)) => {
            for event in went.said() {
                print(event);
            }
            // It exits as `tay down` would, from what the down said, which
            // always says how it ended.
            printer
                .lock()
                .expect("the printer is not poisoned")
                .exit()
                .unwrap_or(Exit::Failed)
        }
        Ok(Served::CannotGoOn(why)) => problem(COMMAND, Exit::Unreachable, &why),
        Err(why) => problem(COMMAND, Exit::Unreachable, &why.to_string()),
    }
}
