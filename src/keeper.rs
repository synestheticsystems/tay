//! The keeper: tay's own binary run as `tay keep`, the parent of one program
//! a unit runs on the host. Only a process's parent is told how it ended; if
//! the daemon were the parent of what it starts, a daemon that died would
//! lose both the exit code and the output of everything it had started. The
//! keeper outlives the daemon, and knows nothing of the stack.
//!
//! The keeper inherits the unit's lock from the daemon that started it and
//! holds it until it ends; it does not pass it on to its child. It writes
//! itself to the unit's record, starts the program as its child in a process
//! group of its own, writes every line the child prints to the unit's log
//! with the time, and when the child ends writes how and when to the record,
//! stops whatever the child left of its group, and ends. The program's
//! environment, which may hold secrets, is the keeper's own, given by the
//! daemon; secrets are never arguments. The daemon names each of its
//! variables, and the child is given those alone, since the system may add
//! variables of its own to a program as it starts, as macOS does to one
//! that uses its frameworks, which tay does to be told of changed files.
//!
//! Two limits are known and accepted. A process that puts itself in a
//! session of its own escapes the stop. And if a keeper is killed, a child
//! that prints dies at its next line, while a silent one lives on; the next
//! daemon finds a record with no lock and no ending, and stops the recorded
//! group.

mod group;
mod process;
mod record;
mod step;
mod stopping;

use std::ffi::OsString;
use std::io::{BufRead, BufReader, PipeReader, Read};
use std::os::fd::{BorrowedFd, RawFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustix::io::{FdFlags, fcntl_setfd};

use crate::clock;
use crate::disk::DiskError;
use crate::disk::unit_log::{Adding, LONGEST_LINE, UnitLog, Writer};

pub use process::RecordedProcess;
pub use record::{Ended, Ending, Record};
pub use step::{Look, Program, StepFiles};

use group::Group;

/// How long the lines a child's group printed have to reach the log once
/// the group has gone: a process that left the group may hold the pipe for
/// ever.
const DRAIN: Duration = Duration::from_secs(1);

/// What `tay keep` is given: the program, and where to keep it.
struct Arguments {
    /// The open file the unit's lock is held on, inherited from the daemon.
    lock: RawFd,
    record: PathBuf,
    log: UnitLog,
    /// What the step is called in its log: `attempt 2, run`.
    name: String,
    /// The names of the variables of its environment the program is given.
    env: Vec<String>,
    argv: Vec<String>,
}

/// Runs the keeper. Its exit code says only whether it could keep the
/// program; how the program ended is in the record.
pub fn keep(args: impl Iterator<Item = OsString>) -> ExitCode {
    let Some(arguments) = read_args(args) else {
        return ExitCode::from(2);
    };
    // SAFETY: the daemon started the keeper with the lock's file open at
    // this number, and nothing in the keeper closes it: it is held until
    // the keeper ends.
    let lock = unsafe { BorrowedFd::borrow_raw(arguments.lock) };
    // The child must not hold the unit's lock, or the lock would say a step
    // runs after its keeper ended.
    if fcntl_setfd(lock, FdFlags::CLOEXEC).is_err() {
        return ExitCode::from(2);
    }
    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// `--lock FD --record FILE --log FOLDER --name NAME [--env VARIABLE]...
/// -- PROGRAM ARGS...`.
fn read_args(mut args: impl Iterator<Item = OsString>) -> Option<Arguments> {
    let (mut lock, mut record, mut log, mut name) = (None, None, None, None);
    let mut env = Vec::new();
    loop {
        let flag = args.next()?;
        if flag == "--" {
            break;
        }
        let value = args.next()?;
        match flag.to_str()? {
            "--lock" => lock = value.to_str()?.parse().ok(),
            "--record" => record = Some(PathBuf::from(value)),
            "--log" => log = Some(UnitLog::at(PathBuf::from(value))),
            "--name" => name = value.into_string().ok(),
            "--env" => env.push(value.into_string().ok()?),
            _ => return None,
        }
    }
    let argv: Vec<String> = args
        .map(|arg| arg.into_string().ok())
        .collect::<Option<_>>()?;
    if argv.is_empty() {
        return None;
    }
    Some(Arguments {
        lock: lock?,
        record: record?,
        log: log?,
        name: name?,
        env,
        argv,
    })
}

fn run(arguments: &Arguments) -> Result<(), DiskError> {
    let mut record = Record::read(&arguments.record)?.ok_or_else(|| DiskError::Unexpected {
        path: arguments.record.clone(),
        found: "nothing".to_string(),
        wanted: "the record the daemon writes before it starts a keeper",
    })?;
    record.keeper = Some(this_process());
    record.write(&arguments.record)?;
    let log = Arc::new(Mutex::new(arguments.log.open()?));
    let shown = arguments.argv.join(" ");
    note(&log, &format!("{}: starting `{shown}`", arguments.name));
    let started = start(&arguments.argv, &arguments.env);
    let (mut child, output) = match started {
        Ok(started) => started,
        Err(error) => {
            let how = Ending::NotStarted {
                why: format!("`{}`: {error}", arguments.argv[0]),
            };
            note(&log, &format!("{}: {how}", arguments.name));
            record.ended = Some(Ended {
                at: clock::now_millis(),
                how,
            });
            return record.write(&arguments.record);
        }
    };
    let leader = RecordedProcess::of(child.id());
    record.child = Some(leader);
    if let Err(error) = record.write(&arguments.record) {
        // A child the record does not name would be lost to every daemon,
        // so it is not left running, nor anything it started. It is not yet
        // waited for, so its group is its own.
        if let Some(group) = rustix::process::Pid::from_raw(leader.pid) {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
        let _ = child.wait();
        note(
            &log,
            &format!(
                "{}: stopped, since its record cannot be written: {error}",
                arguments.name
            ),
        );
        return Err(error);
    }
    let (drained, lines_read) = mpsc::channel();
    let to_log = Arc::clone(&log);
    std::thread::spawn(move || {
        copy_lines(output, &to_log);
        // The keeper may have stopped waiting.
        let _ = drained.send(());
    });
    let status = child.wait();
    let how = match status {
        Ok(status) => match (status.code(), status.signal()) {
            (Some(code), _) => Ending::Code { code },
            (None, Some(signal)) => Ending::Signal { signal },
            (None, None) => Ending::NotStarted {
                why: "the system said neither how it exited nor what ended it".to_string(),
            },
        },
        Err(error) => Ending::NotStarted {
            why: format!("it could not be waited for: {error}"),
        },
    };
    record.ended = Some(Ended {
        at: clock::now_millis(),
        how: how.clone(),
    });
    let recorded = record.write(&arguments.record);
    // Nothing the child started outlives it: what is left of its group is
    // what tay would otherwise lose track of.
    let left = Group::led_by(leader).stop();
    // The keeper may end before the last lines are read, when a process
    // that left the group holds the pipe.
    let _ = lines_read.recv_timeout(DRAIN);
    note(&log, &format!("{}: {how}", arguments.name));
    match left {
        Ok(true) => {}
        Ok(false) => note(
            &log,
            "processes it started were left after they were killed",
        ),
        Err(errno) => note(
            &log,
            &format!("what it started could not be stopped: {errno}"),
        ),
    }
    recorded
}

/// The keeper itself, as its record says it.
fn this_process() -> RecordedProcess {
    RecordedProcess::of(std::process::id())
}

/// Starts `argv` as the keeper's child, leading a process group of its own,
/// with its standard output and error both read from one pipe, so that their
/// lines stay in the order written. It reads the keeper's standard input
/// and runs in the keeper's folder, with the variables `env` names of the
/// keeper's environment.
fn start(argv: &[String], env: &[String]) -> std::io::Result<(std::process::Child, PipeReader)> {
    let (reader, writer) = std::io::pipe()?;
    let mut command = Command::new(&argv[0]);
    command.env_clear();
    for name in env {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .args(&argv[1..])
        .stdin(Stdio::inherit())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .process_group(0);
    let child = command.spawn()?;
    // The keeper keeps no end of the pipe the child writes to, so that it
    // reads to the end once the group has gone.
    drop(command);
    Ok((child, reader))
}

/// Writes each line read from `output` to the log, as the unit's, until the
/// pipe ends.
fn copy_lines(output: PipeReader, log: &Mutex<Adding>) {
    let mut output = BufReader::with_capacity(LONGEST_LINE, output);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut output)
            .take(LONGEST_LINE as u64)
            .read_until(b'\n', &mut line);
        match read {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) => {
                note(log, &format!("what it prints cannot be read: {error}"));
                return;
            }
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        written(log, Writer::Unit, &String::from_utf8_lossy(&line));
    }
}

fn note(log: &Mutex<Adding>, text: &str) {
    written(log, Writer::Tay, text);
}

/// Writes a line to the log, which loses one it cannot write: the keeper's
/// work is to keep the program and record how it ended, which it goes on
/// doing however the log fares.
fn written(log: &Mutex<Adding>, writer: Writer, text: &str) {
    let mut log = log
        .lock()
        .expect("the log is written by one thread at a time");
    log.write(writer, text);
}
