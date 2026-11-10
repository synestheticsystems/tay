//! `tay logs UNIT [--since 10m] [--follow]`: what the unit wrote, each line
//! with the time it was written, and tay's notes among them of where each
//! attempt starts and how it ended. It reads the unit's log files itself,
//! whether or not a daemon runs, and with `--follow` goes on printing what
//! the unit writes until whatever reads it stops.

use std::time::Duration;

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, UnitsNamed};
use crate::cli::stack;
use crate::clock;
use crate::disk::Layout;
use crate::module::Duration as Written;
use crate::print::out_while_read;
use crate::syntax::names;
use crate::unit_log::{Line, UnitLog};

use super::{disk_problem, problem};

pub const TAKES: Takes = Takes {
    command: "tay logs",
    choices: false,
    json: false,
    follows: true,
    units: UnitsNamed::AtMostOne,
};

const COMMAND: &str = "tay logs";

/// How often a log that is followed is read again.
const FOLLOW: Duration = Duration::from_millis(250);

pub fn run(args: Args) -> Exit {
    let [unit] = args.units.as_slice() else {
        return problem(COMMAND, Exit::Errors, "name the unit whose log to read");
    };
    if let Some(why) = names::name_problem(unit) {
        return problem(COMMAND, Exit::Errors, &why);
    }
    let since = match &args.since {
        None => String::new(),
        Some(written) => match Written::parse(written) {
            Some(back) => {
                let back = u64::try_from(back.length().as_millis()).unwrap_or(u64::MAX);
                clock::stamp(clock::now_millis().saturating_sub(back))
            }
            None => {
                return problem(
                    COMMAND,
                    Exit::Errors,
                    &format!(
                        "`--since` is a whole number followed by s, m or h, such as `10m`; this is `{written}`"
                    ),
                );
            }
        },
    };
    let root = match stack::root(args.root.as_deref()) {
        Ok(root) => root,
        Err(message) => return problem(COMMAND, Exit::Errors, &message),
    };
    let log = UnitLog::at(Layout::of(&root).unit_log(unit));
    if !log.folder().exists() {
        return problem(
            COMMAND,
            Exit::Errors,
            &format!(
                "`{unit}` has written nothing in this environment: there is no log at `{}`",
                log.folder().display()
            ),
        );
    }
    let (lines, mut following) = match log.read_since(&since) {
        Ok(read) => read,
        Err(error) => return disk_problem(COMMAND, &error),
    };
    if !print(&lines) || !args.follow {
        return Exit::Done;
    }
    loop {
        std::thread::sleep(FOLLOW);
        let mut lines = Vec::new();
        if let Err(error) = following.read(&since, &mut lines) {
            return disk_problem(COMMAND, &error);
        }
        if !print(&lines) {
            return Exit::Done;
        }
    }
}

/// Prints `lines`; whether anything still reads them.
fn print(lines: &[Line]) -> bool {
    let text: String = lines
        .iter()
        .map(|line| format!("{}\n", line.whole()))
        .collect();
    text.is_empty() || out_while_read(&text)
}
