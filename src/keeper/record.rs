//! A unit's record, `state/units/<unit>/record.json`: what the daemon and the
//! unit's keeper write of the step under way. With the unit's lock, it tells
//! a daemon that did not start the step whether it still runs, how it
//! ended, or that its keeper ended without saying.
//!
//! The daemon writes the attempt and the step as the step begins; the keeper
//! adds itself, then its child, then how the child ended. Each writes the
//! record whole, and never while the other may: the daemon writes before the
//! keeper exists, and the keeper while it holds the unit's lock.

use std::fmt;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::process::RecordedProcess;
use crate::disk::{DiskError, write_whole};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The attempt the step is part of, counted by the daemon.
    pub attempt: u32,
    /// The step, in the words of whatever runs the unit.
    pub step: String,
    /// When the daemon began the step, in milliseconds since 1970.
    pub begun: u64,
    /// The keeper, once it has written itself.
    pub keeper: Option<RecordedProcess>,
    /// The program the keeper started, which leads a process group of its
    /// own.
    pub child: Option<RecordedProcess>,
    pub ended: Option<Ended>,
}

/// How the keeper's child ended, and when, in milliseconds since 1970.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ended {
    pub at: u64,
    pub how: Ending,
}

/// How a program ended: its exit code or the signal that ended it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Ending {
    /// It exited with this code.
    Code { code: i32 },
    /// A signal ended it.
    Signal { signal: i32 },
    /// It could not be started, for this reason.
    NotStarted { why: String },
}

impl Ending {
    /// Whether the program did what it was run for: it exited with 0.
    pub fn succeeded(&self) -> bool {
        matches!(self, Ending::Code { code: 0 })
    }

    /// The program's exit status, as a shell gives it: its code, or 128
    /// and the number of the signal that ended it; `None` when it was not
    /// started.
    pub fn exit_status(&self) -> Option<i64> {
        match self {
            Ending::Code { code } => Some(i64::from(*code)),
            Ending::Signal { signal } => Some(128 + i64::from(*signal)),
            Ending::NotStarted { .. } => None,
        }
    }
}

impl fmt::Display for Ending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ending::Code { code } => write!(f, "exited with {code}"),
            Ending::Signal { signal } => match signal_name(*signal) {
                Some(name) => write!(f, "was killed by signal {signal} ({name})"),
                None => write!(f, "was killed by signal {signal}"),
            },
            Ending::NotStarted { why } => write!(f, "could not be started: {why}"),
        }
    }
}

/// The name of a signal a program is likely to be ended by.
fn signal_name(signal: i32) -> Option<&'static str> {
    use rustix::process::Signal;
    [
        (Signal::HUP, "HUP"),
        (Signal::INT, "INT"),
        (Signal::QUIT, "QUIT"),
        (Signal::ILL, "ILL"),
        (Signal::ABORT, "ABRT"),
        (Signal::BUS, "BUS"),
        (Signal::FPE, "FPE"),
        (Signal::KILL, "KILL"),
        (Signal::SEGV, "SEGV"),
        (Signal::PIPE, "PIPE"),
        (Signal::ALARM, "ALRM"),
        (Signal::TERM, "TERM"),
    ]
    .into_iter()
    .find(|(known, _)| known.as_raw() == signal)
    .map(|(_, name)| name)
}

impl Record {
    /// The record at `path`; `None` when there is none.
    pub fn read(path: &Path) -> Result<Option<Record>, DiskError> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                serde_json::from_str(&text)
                    .map(Some)
                    .map_err(|source| DiskError::Unreadable {
                        path: path.to_path_buf(),
                        source,
                    })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(DiskError::io(path, "read")(error)),
        }
    }

    pub fn write(&self, path: &Path) -> Result<(), DiskError> {
        let json = serde_json::to_string_pretty(self).expect("a record serializes");
        write_whole(path, format!("{json}\n").as_bytes(), 0o600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exit_says_how_the_program_ended() {
        assert_eq!(Ending::Code { code: 7 }.to_string(), "exited with 7");
        assert_eq!(
            Ending::Signal { signal: 9 }.to_string(),
            "was killed by signal 9 (KILL)"
        );
        assert!(Ending::Code { code: 0 }.succeeded());
        assert!(!Ending::Signal { signal: 15 }.succeeded());
    }

    #[test]
    fn a_record_is_written_whole_and_read_back() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("record.json");
        assert_eq!(Record::read(&path).unwrap(), None);
        let record = Record {
            attempt: 2,
            step: "run".to_string(),
            begun: 1_794_292_971_005,
            keeper: Some(RecordedProcess {
                pid: 41,
                started: Some(1),
            }),
            child: None,
            ended: Some(Ended {
                at: 1_794_292_972_000,
                how: Ending::NotStarted {
                    why: "No such file or directory".to_string(),
                },
            }),
        };
        record.write(&path).unwrap();
        assert_eq!(Record::read(&path).unwrap(), Some(record));
    }
}
