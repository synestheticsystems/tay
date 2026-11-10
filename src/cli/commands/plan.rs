//! `tay plan`, and planning as it does for the commands that plan first.

use std::path::{Path, PathBuf};

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, UnitsNamed};
use crate::cli::output::{json, text};
use crate::cli::stack;
use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Sources;
use crate::disk;
use crate::plan::{self, Options, Outcome, Plan};
use crate::print::{err, out};

pub const TAKES: Takes = Takes {
    command: "tay plan",
    choices: true,
    json: true,
    units: UnitsNamed::None,
};

pub fn run(args: Args) -> Exit {
    let root = match stack::root(args.root.as_deref()) {
        Ok(root) => root,
        Err(message) => return no_stack("tay plan", &message, args.json),
    };
    match planned(
        "tay plan",
        &root,
        &options(&args),
        args.json,
        "there is no plan",
    ) {
        Ok(plan) => {
            if args.json {
                print_json(&json::plan(&plan));
            } else {
                out(&text::plan(&plan));
            }
            Exit::Done
        }
        Err(exit) => exit,
    }
}

/// What a command that plans as `tay plan` does plans with.
pub fn options(args: &Args) -> Options {
    Options {
        profile: args.profile.clone(),
        sets: args.sets.clone(),
        units: args.units.clone(),
        home: std::env::var_os("HOME").map(PathBuf::from),
        shared_folders: disk::shared_folders(),
    }
}

/// Plans the stack at `root` as `tay plan` does. When the definitions have
/// errors, prints them as `tay plan` does, `command` closing them with
/// what that means, and gives how the command ends.
pub fn planned(
    command: &str,
    root: &Path,
    options: &Options,
    json: bool,
    closing: &str,
) -> Result<Plan, Exit> {
    let mut sources = Sources::default();
    let mut d = Diagnostics::default();
    match plan::plan(root, options, &mut sources, &mut d) {
        Outcome::Plan(plan) => Ok(plan),
        Outcome::Errors => {
            let errors = d.into_sorted();
            if json {
                print_json(&json::errors("errors", &errors, &sources));
            } else {
                for error in &errors {
                    err(&format!("{}\n", error.render(&sources)));
                }
                let count = match errors.len() {
                    1 => "1 error".to_string(),
                    n => format!("{n} errors"),
                };
                err(&format!(
                    "{command}: {count} in the definitions; {closing}.\n"
                ));
            }
            Err(Exit::Errors)
        }
        Outcome::TooOld(reasons) => {
            if json {
                print_json(&json::errors("too_old", &reasons, &sources));
            } else {
                for reason in &reasons {
                    err(&format!("{}\n", reason.render(&sources)));
                }
            }
            Err(Exit::TooOld)
        }
    }
}

/// No stack was found to plan: said as `tay plan` says it.
pub fn no_stack(command: &str, message: &str, json: bool) -> Exit {
    if json {
        print_json(&json::error_without_place(message));
    } else {
        err(&format!("{command}: {message}\n"));
    }
    Exit::Errors
}

pub fn print_json(value: &serde_json::Value) {
    let json = serde_json::to_string_pretty(value).expect("a JSON value always serializes");
    out(&format!("{json}\n"));
}
