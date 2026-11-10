//! `tay unset PLACE...`: forgets what the environment remembers being asked
//! for in each place, so that the profile's or the stack's value holds
//! again, and touches no unit until the next `tay up`. A place with nothing
//! remembered is an error, and nothing is forgotten. What is remembered is
//! listed afterwards, as `tay set` lists it.

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, Words};
use crate::cli::stack;
use crate::control::protocol::Remember;
use crate::disk::{Asked, Layout};

use super::plan::no_stack;
use super::set::remember;
use super::{disk_problem, problem};

pub const TAKES: Takes = Takes {
    command: "tay unset",
    choices: false,
    json: true,
    follows: false,
    wipes: false,
    words: Words::Any,
};

const COMMAND: &str = "tay unset";

pub fn run(args: Args) -> Exit {
    let root = match stack::root(args.root.as_deref()) {
        Ok(root) => root,
        Err(message) => return no_stack(COMMAND, &message, args.json),
    };
    if args.words.is_empty() {
        return problem(
            COMMAND,
            Exit::Errors,
            "name each place to forget, as `settings.NAME`",
        );
    }
    let remembered = match Asked::read(&Layout::of(&root)) {
        Ok(remembered) => remembered,
        Err(error) => return disk_problem(COMMAND, &error),
    };
    for place in &args.words {
        if place.contains('=') {
            return problem(
                COMMAND,
                Exit::Errors,
                // The value is not quoted: it may be a secret.
                &format!(
                    "`{}=` gives a value; `tay unset` takes only the place, as `settings.NAME`",
                    Asked::place(place)
                ),
            );
        }
        if !remembered.remembers(place) {
            return problem(
                COMMAND,
                Exit::Errors,
                &format!("nothing is remembered for `{place}`, so nothing was forgotten"),
            );
        }
    }
    remember(
        COMMAND,
        &root,
        &Remember::Unset { places: args.words },
        args.json,
    )
}
