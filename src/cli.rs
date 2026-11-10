//! The command line: `tay plan`, `tay up`, `tay down`, `tay status`,
//! `tay ls`, `tay --version` and `tay --help`, and `tay daemon`, which a
//! command runs to start an environment's daemon.

mod args;
mod commands;
mod output;
mod stack;

use std::ffi::OsString;
use std::process::ExitCode;

use crate::control::daemon;
use crate::definition::read::THIS_TAY;
use crate::print::{err, out};

use args::{Takes, UnitsNamed};
use commands::{down, ls, plan, status, up};

/// How a command ends, as its exit code says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// Done: the plan is sound, everything asked for is ready, or nothing
    /// is left running.
    Done = 0,
    /// A unit failed, or something was left that could not be stopped.
    Failed = 1,
    /// The definitions, or the command line, have errors.
    Errors = 2,
    /// This tay is too old for the definitions.
    TooOld = 3,
    /// Refused: another change is running.
    Refused = 4,
    /// The daemon could not be started or reached.
    Unreachable = 5,
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> ExitCode {
        ExitCode::from(exit as u8)
    }
}

const HELP: &str = "\
tay (Topology Automation via YAML) brings a stack of host processes and
containers up in dependency order and reports their state. This version makes
a stack's ports, secrets and files; it cannot run a process, a container or a
command yet, and says so of each unit that would.

Usage:
  tay plan [--root FOLDER] [--profile NAME] [--set PLACE=VALUE]... [--json]
  tay up [UNIT...] [--root FOLDER] [--profile NAME] [--set PLACE=VALUE]... [--json]
  tay down [--root FOLDER] [--json]
  tay status [UNIT] [--root FOLDER] [--json]
  tay ls [--root FOLDER] [--json]
  tay --version
  tay --help

tay plan reads the stack whose tay.yaml is in the current folder or the
nearest folder above it, applies the choices, and prints each unit that would
come up, in an order it could start in: the module it uses and where it runs,
what it needs and why, how it is known to be ready, the host ports it opens
and what opens each, and the values it gives. Of the environment's .tay/
folder it reads choices.yaml and nothing else, and it writes nothing.

tay up plans exactly as tay plan does, starts the environment's daemon if none
runs, and has it bring up what the choices list under `up`, or the units named
and what they need. It prints each change of state as it happens, and returns
when everything asked for is ready or one of it can no longer become ready.
--profile and --set apply to that tay up only, for now: a daemon started
afresh plans with the stack's own choices.

tay down stops everything the environment runs and says so; its daemon exits.
tay status prints each unit's state, from a check made now. tay ls prints the
environments tay knows on this machine, and marks with * the one it is run in.

Options:
  --root FOLDER       look for tay.yaml from FOLDER instead of the current one
  --profile NAME      of plan and up: use this profile instead of the stack's
                      default one
  --set PLACE=VALUE   of plan and up: make one choice, after every other
                      place. PLACE is up, up_add, up_remove, run.default,
                      run.units.UNIT, settings.NAME or label; a list is
                      written with commas
  --json              print for programs, in schema 1: the plan, each event of
                      up and down as a JSON object on a line, the units of
                      status, and the environments of ls

Exit codes: 0 done: the plan is sound, everything asked for is ready, or
nothing is left running; 1 a unit failed, or something was left that could
not be stopped; 2 the definitions or the command line have errors; 3 this tay
is too old for the definitions; 4 refused: another change is running; 5 the
daemon could not be started or reached.
";

/// What `tay daemon` takes: the stack's root, which a command gives it.
const DAEMON: Takes = Takes {
    command: "tay daemon",
    choices: false,
    json: false,
    units: UnitsNamed::None,
};

pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let mut args = args.into_iter();
    let Some(verb) = args.next() else {
        err(HELP);
        return Exit::Errors.into();
    };
    let (takes, command): (&Takes, fn(args::Args) -> Exit) = match verb.to_str() {
        Some("--help" | "-h" | "help") => {
            out(HELP);
            return Exit::Done.into();
        }
        Some("--version" | "-V") => {
            out(&format!("tay {THIS_TAY}\n"));
            return Exit::Done.into();
        }
        Some("plan") => (&plan::TAKES, plan::run),
        Some("up") => (&up::TAKES, up::run),
        Some("down") => (&down::TAKES, down::run),
        Some("status") => (&status::TAKES, status::run),
        Some("ls") => (&ls::TAKES, ls::run),
        Some("daemon") => (&DAEMON, run_daemon),
        Some("logs") => {
            err(
                "tay logs: not built yet. This version of tay runs no unit, so no unit writes anything.\n",
            );
            return Exit::Errors.into();
        }
        Some(other) if other.starts_with('-') => {
            err(&format!(
                "tay: unknown option `{other}`\nRun `tay --help` for how to use it.\n"
            ));
            return Exit::Errors.into();
        }
        _ => {
            err(&format!(
                "tay: `{}` is not a command\nRun `tay --help` for how to use it.\n",
                verb.to_string_lossy()
            ));
            return Exit::Errors.into();
        }
    };
    match takes.read(args) {
        Ok(Some(args)) => command(args).into(),
        Ok(None) => {
            out(HELP);
            Exit::Done.into()
        }
        Err(message) => {
            err(&format!(
                "{}: {message}\nRun `tay --help` for how to use it.\n",
                takes.command
            ));
            Exit::Errors.into()
        }
    }
}

fn run_daemon(args: args::Args) -> Exit {
    let Some(root) = args.root else {
        err("tay daemon: `--root` is required: it is run by the command that starts it\n");
        return Exit::Errors;
    };
    match daemon::run(&root) {
        true => Exit::Done,
        false => Exit::Failed,
    }
}
