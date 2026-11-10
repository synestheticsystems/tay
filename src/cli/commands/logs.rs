//! `tay logs [UNIT...] [--since 10m] [--follow]`: what the units wrote, each
//! line with the time it was written, and tay's notes among them of where
//! each attempt starts and how it ended. With one unit named, its lines are
//! given as its log holds them. With several, or none, which reads every
//! unit that has written anything, the lines of all are given in the order
//! of their times, each naming its unit after its time. It reads the units'
//! log files itself, whether or not a daemon runs, and with `--follow` goes
//! on printing what they write until whatever reads it stops; following
//! every unit takes in each that begins to write meanwhile.

use std::time::Duration;

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, Words};
use crate::cli::stack;
use crate::clock;
use crate::disk::unit_log::{self, Following, Line, UnitLog};
use crate::disk::{DiskError, Layout};
use crate::module::Duration as Written;
use crate::print::out_while_read;
use crate::syntax::names;

use super::{disk_problem, problem};

pub const TAKES: Takes = Takes {
    command: "tay logs",
    choices: false,
    json: false,
    follows: true,
    words: Words::Any,
};

const COMMAND: &str = "tay logs";

/// How often a log that is followed is read again.
const FOLLOW: Duration = Duration::from_millis(250);

pub fn run(args: Args) -> Exit {
    if let Some(why) = args.words.iter().find_map(|unit| names::name_problem(unit)) {
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
    let layout = Layout::of(&root);
    for unit in &args.words {
        let folder = layout.unit_log(unit);
        if !folder.exists() {
            return problem(
                COMMAND,
                Exit::Errors,
                &format!(
                    "`{unit}` has written nothing in this environment: there is no log at `{}`",
                    folder.display()
                ),
            );
        }
    }
    let mut reading = Reading {
        layout,
        every_unit: args.words.is_empty(),
        naming: args.words.len() != 1,
        since,
        width: 0,
        following: Vec::new(),
    };
    let first = match reading.begin(&args.words) {
        Ok(lines) => lines,
        Err(error) => return disk_problem(COMMAND, &error),
    };
    if !reading.print(first) || !args.follow {
        return Exit::Done;
    }
    loop {
        std::thread::sleep(FOLLOW);
        let lines = match reading.more() {
            Ok(lines) => lines,
            Err(error) => return disk_problem(COMMAND, &error),
        };
        if !reading.print(lines) {
            return Exit::Done;
        }
    }
}

/// The logs of the units being read, each followed from where it was read
/// to.
struct Reading {
    layout: Layout,
    /// Whether every unit that has written anything is read, rather than
    /// those named.
    every_unit: bool,
    /// Whether each line names its unit, as it does among several.
    naming: bool,
    /// The earliest time a line is given from, as the log writes times.
    since: String,
    /// The longest name of a unit read so far, which each name is padded
    /// to.
    width: usize,
    following: Vec<(String, Following)>,
}

impl Reading {
    /// Reads `units`, or every unit that has written anything when there
    /// are none, from `since`: their lines in the order of their times.
    fn begin(&mut self, units: &[String]) -> Result<Vec<(String, Line)>, DiskError> {
        let units = match self.every_unit {
            true => unit_log::units_with_logs(&self.layout.logs())?,
            false => units.to_vec(),
        };
        let mut lines = Vec::new();
        for unit in units {
            lines.extend(self.take_in(unit)?);
        }
        Ok(in_order(lines))
    }

    /// What the units read have written since they were last read, and,
    /// when every unit is read, all of each that has begun to write
    /// meanwhile: in the order of their times.
    fn more(&mut self) -> Result<Vec<(String, Line)>, DiskError> {
        let mut lines = Vec::new();
        for (unit, following) in &mut self.following {
            let mut gained = Vec::new();
            following.read(&self.since, &mut gained)?;
            lines.extend(gained.into_iter().map(|line| (unit.clone(), line)));
        }
        if self.every_unit {
            for unit in unit_log::units_with_logs(&self.layout.logs())? {
                if !self.following.iter().any(|(read, _)| *read == unit) {
                    lines.extend(self.take_in(unit)?);
                }
            }
        }
        Ok(in_order(lines))
    }

    /// Reads `unit`'s log from `since`, and follows it from there.
    fn take_in(&mut self, unit: String) -> Result<Vec<(String, Line)>, DiskError> {
        let log = UnitLog::at(self.layout.unit_log(&unit));
        let (lines, following) = log.read_since(&self.since)?;
        self.width = self.width.max(unit.len());
        let lines = lines.into_iter().map(|line| (unit.clone(), line)).collect();
        self.following.push((unit, following));
        Ok(lines)
    }

    /// Prints `lines`; whether anything still reads them.
    fn print(&self, lines: Vec<(String, Line)>) -> bool {
        let text: String = lines
            .into_iter()
            .map(|(unit, line)| match self.naming {
                true => format!(
                    "{} {unit:<width$} {} {}\n",
                    line.time,
                    line.writer.marker(),
                    line.text,
                    width = self.width
                ),
                false => format!("{}\n", line.whole()),
            })
            .collect();
        text.is_empty() || out_while_read(&text)
    }
}

/// `lines` in the order of their times; the lines of one time in the order
/// they were read, which for one unit is the order it wrote them in.
fn in_order(mut lines: Vec<(String, Line)>) -> Vec<(String, Line)> {
    lines.sort_by(|(_, one), (_, other)| one.time.cmp(&other.time));
    lines
}
