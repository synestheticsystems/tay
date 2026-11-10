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
//! daemon; secrets are never arguments.
//!
//! Two limits are known and accepted. A process that puts itself in a
//! session of its own escapes the stop. And if a keeper is killed, a child
//! that prints dies at its next line, while a silent one lives on; the next
//! daemon finds a record with no lock and no ending, and stops the recorded
//! group.

mod group;
mod launch;
mod record;
mod started;

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
use crate::unit_log::{Adding, LONGEST_LINE, UnitLog, Writer};

pub use launch::{Look, Program, UnitFiles};
pub use record::{Ended, Exit, Process, Record};

use group::Group;

/// How long the lines a child's group printed have to reach the log once
/// the group has gone: a process that left the group may hold the pipe for
/// ever.
const DRAIN: Duration = Duration::from_secs(1);

/// What `tay keep` is asked to keep.
struct Asked {
    /// The open file the unit's lock is held on, inherited from the daemon.
    lock: RawFd,
    record: PathBuf,
    log: UnitLog,
    /// What the step is called in its log: `attempt 2, run`.
    name: String,
    argv: Vec<String>,
}

/// Runs the keeper. Its exit code says only whether it could keep the
/// program; how the program ended is in the record.
pub fn keep(args: impl Iterator<Item = OsString>) -> ExitCode {
    let Some(asked) = read_args(args) else {
        return ExitCode::from(2);
    };
    // SAFETY: the daemon started the keeper with the lock's file open at
    // this number, and nothing in the keeper closes it: it is held until
    // the keeper ends.
    let lock = unsafe { BorrowedFd::borrow_raw(asked.lock) };
    // The child must not hold the unit's lock, or the lock would say a step
    // runs after its keeper ended.
    if fcntl_setfd(lock, FdFlags::CLOEXEC).is_err() {
        return ExitCode::from(2);
    }
    match run(&asked) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// `--lock FD --record FILE --log FOLDER --name NAME -- PROGRAM ARGS...`.
fn read_args(mut args: impl Iterator<Item = OsString>) -> Option<Asked> {
    let (mut lock, mut record, mut log, mut name) = (None, None, None, None);
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
            _ => return None,
        }
    }
    let argv: Vec<String> = args
        .map(|arg| arg.into_string().ok())
        .collect::<Option<_>>()?;
    if argv.is_empty() {
        return None;
    }
    Some(Asked {
        lock: lock?,
        record: record?,
        log: log?,
        name: name?,
        argv,
    })
}

fn run(asked: &Asked) -> Result<(), DiskError> {
    let mut record = Record::read(&asked.record)?.ok_or_else(|| DiskError::Unexpected {
        path: asked.record.clone(),
        found: "nothing".to_string(),
        wanted: "the record the daemon writes before it starts a keeper",
    })?;
    record.keeper = Some(this_process());
    record.write(&asked.record)?;
    let log = Arc::new(Mutex::new(asked.log.open()?));
    let shown = asked.argv.join(" ");
    note(&log, &format!("{}: starting `{shown}`", asked.name));
    let started = start(&asked.argv);
    let (mut child, output) = match started {
        Ok(started) => started,
        Err(error) => {
            let exit = Exit::NotStarted {
                why: format!("`{}`: {error}", asked.argv[0]),
            };
            note(&log, &format!("{}: {exit}", asked.name));
            record.ended = Some(Ended {
                at: clock::now_millis(),
                exit,
            });
            return record.write(&asked.record);
        }
    };
    let leader = process(child.id());
    record.child = Some(leader);
    if let Err(error) = record.write(&asked.record) {
        // A child the record does not name would be lost to every daemon,
        // so it is not left running.
        let _ = child.kill();
        let _ = child.wait();
        note(
            &log,
            &format!(
                "{}: stopped, since its record cannot be written: {error}",
                asked.name
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
    let exit = match status {
        Ok(status) => match (status.code(), status.signal()) {
            (Some(code), _) => Exit::Code { code },
            (None, Some(signal)) => Exit::Signal { signal },
            (None, None) => Exit::NotStarted {
                why: "the system said neither how it exited nor what ended it".to_string(),
            },
        },
        Err(error) => Exit::NotStarted {
            why: format!("it could not be waited for: {error}"),
        },
    };
    record.ended = Some(Ended {
        at: clock::now_millis(),
        exit: exit.clone(),
    });
    let recorded = record.write(&asked.record);
    // Nothing the child started outlives it: what is left of its group is
    // what tay would otherwise lose track of.
    let left = Group::led_by(leader).stop();
    // The keeper may end before the last lines are read, when a process
    // that left the group holds the pipe.
    let _ = lines_read.recv_timeout(DRAIN);
    note(&log, &format!("{}: {exit}", asked.name));
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

/// A process the keeper knows: itself, or its child before it is waited for.
fn process(pid: u32) -> Process {
    let pid = i32::try_from(pid).expect("the system's process numbers fit in an i32");
    Process {
        pid,
        started: started::started(pid),
    }
}

/// The keeper itself, as its record says it.
fn this_process() -> Process {
    process(std::process::id())
}

/// Starts `argv` as the keeper's child, leading a process group of its own,
/// with its standard output and error both read from one pipe, so that their
/// lines stay in the order written. It reads the keeper's standard input
/// and runs in the keeper's folder, with the keeper's environment.
fn start(argv: &[String]) -> std::io::Result<(std::process::Child, PipeReader)> {
    let (reader, writer) = std::io::pipe()?;
    let mut command = Command::new(&argv[0]);
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
            Ok(0) | Err(_) => return,
            Ok(_) => {}
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

/// Writes a line to the log. A line that cannot be written is lost: the
/// keeper's work is to keep the program and record how it ended, which it
/// goes on doing however the log fares, and it has nowhere else to say so.
fn written(log: &Mutex<Adding>, writer: Writer, text: &str) {
    let mut log = log
        .lock()
        .expect("the log is written by one thread at a time");
    let _ = log.write(writer, text);
}
