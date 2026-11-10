//! The command line: `tay plan`, `tay up`, `tay down`, `tay status`,
//! `tay logs`, `tay ls`, `tay restart`, `tay run`, `tay set`, `tay unset`,
//! `tay wipe`, `tay --version` and `tay --help`; `tay daemon`,
//! which a command runs to start an environment's daemon; and `tay keep`,
//! which the daemon runs to keep one program of a unit.

mod args;
mod commands;
mod output;
mod stack;

use std::ffi::OsString;
use std::process::ExitCode;

use crate::control::daemon;
use crate::definition::read::THIS_TAY;
use crate::keeper;
use crate::print::{err, out};

use args::{Takes, Words};
use commands::{down, logs, ls, plan, restart, run, set, status, unset, up, wipe};

/// How a command ends, as its exit code says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// Done: the plan is sound, everything asked for is ready, or nothing
    /// is left running.
    Done,
    /// A unit failed, or something was left that could not be stopped.
    Failed,
    /// The definitions have errors, or what was asked does not fit the
    /// stack.
    Errors,
    /// This tay is too old for the definitions.
    TooOld,
    /// Refused as things stand: another change is running, or what was
    /// asked needs something that is not up.
    Refused,
    /// The daemon could not be started or reached.
    Unreachable,
    /// The action `tay run` ran exited with this code, which is not 0.
    Action(u8),
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> ExitCode {
        ExitCode::from(match exit {
            Exit::Done => 0,
            Exit::Failed => 1,
            Exit::Errors => 2,
            Exit::TooOld => 3,
            Exit::Refused => 4,
            Exit::Unreachable => 5,
            Exit::Action(code) => code,
        })
    }
}

const HELP: &str = "\
tay (Topology Automation via YAML) brings a stack of host processes and
containers up in dependency order and reports their state. It runs processes
and commands on the host, each under a keeper that outlives the daemon, runs
containers and commands in them through Docker, and makes a stack's ports,
secrets and files.

Usage:
  tay plan [--root FOLDER] [--profile NAME] [--set PLACE=VALUE]... [--json]
  tay up [UNIT...] [--root FOLDER] [--profile NAME] [--set PLACE=VALUE]... [--json]
  tay down [--root FOLDER] [--json]
  tay status [UNIT] [--root FOLDER] [--json]
  tay logs [UNIT...] [--root FOLDER] [--since TIME] [--follow]
  tay ls [--root FOLDER] [--json]
  tay restart UNIT... [--root FOLDER] [--json]
  tay run ACTION [NAME=VALUE...] [--root FOLDER] [--json]
  tay set [PLACE=VALUE...] [--root FOLDER] [--json]
  tay unset PLACE... [--root FOLDER] [--json]
  tay wipe UNIT... [--yes] [--root FOLDER] [--json]
  tay wipe --all [--yes] [--root FOLDER] [--json]
  tay --version
  tay --help

tay plan reads the stack whose tay.yaml is in the current folder or the
nearest folder above it, applies the choices, and prints each unit that would
come up, in an order it could start in: the module it uses and where it runs,
what it needs and why, how it is known to be ready, the host ports it opens
and what opens each, and the values it gives. Of the environment's .tay/
folder it reads choices.yaml and what the environment remembers being asked
for, and it writes nothing.

tay up plans exactly as tay plan does, starts the environment's daemon if none
runs, and has it bring up what the choices list under `up`, or the units named
and what they need. What runs and is not in the plan any more is stopped, and
it says so. It prints each change of state as it happens, and returns when
everything asked for is ready or one of it can no longer become ready.
The environment remembers the profile, each --set and the units named, so a
later plain tay up, tay status, tay plan and a daemon started afresh all see
the same stack: --profile replaces the remembered profile, a --set replaces a
remembered one for the same place, and units named replace those remembered.

tay down stops everything the environment runs, checks that nothing is left,
and says so; its daemon exits. tay status prints each unit's state, from a
check made now, each unit that runs and is not in the plan any more, and what
the environment remembers being asked for. tay logs prints what a unit wrote,
each line with its time, and tay's notes of where each attempt starts and how
it ended; with several units named, or none, which is every unit that has
written anything, it prints the lines of all in the order of their times, each
naming its unit. tay ls prints the environments tay knows on this machine, and
marks with * the one it is run in.

tay restart stops each unit named and starts it again, with a new first
attempt, bringing up what it needs as tay up would, and leaving the units that
need it as they run; a command is run again from its check. A unit that is
made has nothing to restart, and neither has an environment that is down.

tay run runs a unit that has `action: true`, given each argument it
declares as NAME=VALUE, after bringing up what it needs as tay up would, on
an environment that is up. An argument it does not declare, one of another
type than its default, or a missing one that has no default, is an error
before anything runs. It prints what the action writes as it writes it,
then each value the action produced, by name, a secret one as <secret>, and
it exits as the action did. Nothing the action gave is kept.

tay set changes what the environment remembers being asked for, in the places
--set takes, and touches no unit until the next tay up: tay up --set is the
same thing followed by up. Each is checked as tay up --set checks it, and
nothing is remembered of a stack that does not plan with it. tay unset forgets
what is remembered for each place, so that the profile's or the stack's value
holds again. Both list what is remembered afterwards, as tay set with nothing
after it does; the value of a secret setting is never printed.

tay wipe removes what each unit named keeps, so that it starts from nothing:
a container's volumes, what a command's undo undoes and the values it
produced, a stored secret, a written file, a kept port number, and the
record of a unit that runs. Each running unit that needs one is stopped
first, then the unit, and on an environment meant to be up everything is
brought up again as tay up would, so that commands check and run again. A
command whose undo needs a unit that is not up is refused. tay wipe --all
takes the environment down and removes everything it keeps: every volume of
it, and .tay/ but for choices.yaml. Without --yes nothing changes, and it
says what it would remove.

Options:
  --root FOLDER       look for tay.yaml from FOLDER instead of the current one
  --profile NAME      of plan and up: use this profile instead of the one
                      remembered or the stack's default one
  --set PLACE=VALUE   of plan and up: make one choice, after every other
                      place. PLACE is up, up_add, up_remove, run.default,
                      run.units.UNIT, settings.NAME or label; a list is
                      written with commas
  --json              print for programs, in schema 1: the plan, each event of
                      up, down, restart, run and wipe as a JSON object on a
                      line, the units of status, the environments of ls,
                      what set and unset list, and what wipe would remove
  --yes               of wipe: remove what is kept, rather than say what it is
  --all               of wipe: everything the environment keeps
  --since TIME        of logs: only what was written in the last TIME, a
                      whole number followed by s, m or h, such as 10m
  --follow            of logs: go on printing what the units write

Exit codes: 0 done: the plan is sound, everything asked for is ready, or
nothing is left running; 1 a unit failed, or something was left that could
not be stopped; 2 the definitions or the command line have errors, or what
was asked does not fit the stack; 3 this tay is too old for the definitions;
4 refused as things stand: another change is running, or what was asked
needs something that is not up; 5 the daemon could not be started or
reached.
";

/// What `tay daemon` takes: the stack's root, which a command gives it.
const DAEMON: Takes = Takes {
    command: "tay daemon",
    choices: false,
    json: false,
    follows: false,
    wipes: false,
    words: Words::None,
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
        Some("logs") => (&logs::TAKES, logs::run),
        Some("restart") => (&restart::TAKES, restart::run),
        Some("run") => (&run::TAKES, run::run),
        Some("set") => (&set::TAKES, set::run),
        Some("unset") => (&unset::TAKES, unset::run),
        Some("wipe") => (&wipe::TAKES, wipe::run),
        Some("keep") => return keeper::keep(args),
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
