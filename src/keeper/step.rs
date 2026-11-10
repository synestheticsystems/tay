//! The daemon's side of a keeper: the files a unit's steps are kept with,
//! starting a keeper for a step under the unit's lock, looking at the lock
//! and the record to find a step again, and waiting for a step to end.
//!
//! The order of a start closes every gap: the daemon writes the step to the
//! unit's record, takes the unit's lock, and starts the keeper, which
//! inherits the lock. The lock is held without a gap from before the keeper
//! exists until it ends, and the system releases it if the keeper dies.

use std::fs::{DirBuilder, File};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use rustix::io::{FdFlags, fcntl_setfd};

use super::group::Group;
use super::record::{Ended, Record};
use super::stopping;
use crate::clock;
use crate::disk::unit_log::UnitLog;
use crate::disk::{DiskError, Lock};

/// How often a daemon looks at a step under way for its end.
pub(super) const LOOK: Duration = Duration::from_millis(250);

/// How long a start waits for a lock that a look holds for a moment.
const LOOKED_AT: Duration = Duration::from_millis(200);

/// The files the steps of one unit are kept with: a folder for its lock and
/// its record, readable by this account only, and its log.
#[derive(Clone, Debug)]
pub struct StepFiles {
    folder: PathBuf,
    pub log: UnitLog,
}

/// What a keeper is to run: the program and its arguments, the whole
/// environment it runs with, the folder it runs in, and the file it reads
/// on its standard input. It has no `Debug`, since the environment may hold
/// secrets.
pub struct Program {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub workdir: PathBuf,
    pub stdin: Option<PathBuf>,
}

/// A unit's step, as its lock and its record say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Look {
    /// No step of it has been begun here.
    Never,
    /// Its keeper holds its lock: the step runs. The record may not yet
    /// say what the keeper started.
    Running(Record),
    /// The step ended, and its keeper said how.
    Ended(Record, Ended),
    /// The lock is free and the record says no ending: the keeper ended
    /// without saying how the step did.
    Lost(Record),
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("it is running already, under a keeper this daemon did not start")]
    Running,
    #[error("`{}` cannot be used as its working folder: {source}", path.display())]
    Workdir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("its `stdin`, `{}`, cannot be read: {source}", path.display())]
    Stdin {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("tay's own binary cannot be found to start a keeper: {0}")]
    NoBinary(std::io::Error),
    #[error("its keeper cannot be started: {0}")]
    Spawn(std::io::Error),
    #[error("its keeper did not end once its processes were stopped")]
    KeeperStays,
    #[error("its folder `{}` cannot be made, since the folder it is in has gone", .0.display())]
    FolderGone(PathBuf),
    #[error("processes of its group {group} were left after they were killed")]
    GroupStays { group: i32 },
    #[error("its processes cannot be signalled: {0}")]
    Signal(rustix::io::Errno),
    #[error(transparent)]
    Disk(#[from] DiskError),
}

impl StepFiles {
    /// The files kept in `folder`, the log in `log`.
    pub fn at(folder: PathBuf, log: PathBuf) -> StepFiles {
        StepFiles {
            folder,
            log: UnitLog::at(log),
        }
    }

    /// The folder of the lock and the record, where what the unit produces
    /// is kept beside them.
    pub fn folder(&self) -> &PathBuf {
        &self.folder
    }

    pub(super) fn lock(&self) -> PathBuf {
        self.folder.join("lock")
    }

    fn record(&self) -> PathBuf {
        self.folder.join("record.json")
    }

    /// Makes the folder, for this account alone, and the one it is in, but
    /// no folder above: a step is never begun in an environment whose
    /// folder has gone, which would make it again.
    fn make(&self) -> Result<(), LaunchError> {
        let above = self.folder.parent().expect("the folder is in a folder");
        for folder in [above, self.folder.as_path()] {
            match DirBuilder::new().mode(0o700).create(folder) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(LaunchError::FolderGone(self.folder.clone()));
                }
                Err(error) => return Err(DiskError::io(folder, "made")(error).into()),
            }
        }
        Ok(())
    }

    /// The unit's step as its lock and its record say now.
    pub fn look(&self) -> Result<Look, DiskError> {
        if !self.folder.exists() {
            return Ok(Look::Never);
        }
        let held = Lock::is_held(&self.lock())?;
        let record = Record::read(&self.record())?;
        Ok(match (held, record) {
            (true, Some(record)) => {
                if let Some(child) = record.child {
                    stopping::remember(&self.folder, child);
                }
                Look::Running(record)
            }
            (false, Some(record)) => match record.ended.clone() {
                Some(ended) => Look::Ended(record, ended),
                None => Look::Lost(record),
            },
            // The record is written before a keeper is started, so a held
            // lock with none is a keeper whose record was removed: what it
            // runs cannot be known.
            (true, None) => {
                return Err(DiskError::Unexpected {
                    path: self.record(),
                    found: "nothing".to_string(),
                    wanted: "the record of the step its keeper runs",
                });
            }
            (false, None) => Look::Never,
        })
    }

    /// Whether anything of the unit runs: a step under way, or what is left
    /// of the group of a step whose keeper was lost.
    pub fn anything_left(&self) -> Result<bool, DiskError> {
        Ok(match self.look()? {
            Look::Running(_) => true,
            Look::Lost(Record {
                child: Some(child), ..
            }) => Group::led_by(child).is_left(),
            Look::Never | Look::Ended(..) | Look::Lost(_) => false,
        })
    }

    /// Begins `step` of attempt `attempt`: writes it to the record, takes
    /// the unit's lock, and starts a keeper for `program`, which inherits
    /// the lock. A unit whose lock is held is running already, and is not
    /// started again. It takes no longer than starting a process takes.
    pub fn start(&self, attempt: u32, step: &str, program: &Program) -> Result<(), LaunchError> {
        // What would stop the keeper starting its child is found first, so
        // that a record is written only of a step that is begun.
        std::fs::metadata(&program.workdir)
            .and_then(|metadata| match metadata.is_dir() {
                true => Ok(()),
                false => Err(std::io::Error::other("it is not a folder")),
            })
            .map_err(|source| LaunchError::Workdir {
                path: program.workdir.clone(),
                source,
            })?;
        let stdin = match &program.stdin {
            Some(path) => Stdio::from(File::open(path).map_err(|source| LaunchError::Stdin {
                path: path.clone(),
                source,
            })?),
            None => Stdio::null(),
        };
        self.make()?;
        // While a keeper holds the lock the record is its to write, so the
        // lock is tried, and let go, before the record is written. A look
        // holds it for a moment only.
        if Lock::try_take_within(&self.lock(), LOOKED_AT)?.is_none() {
            return Err(LaunchError::Running);
        }
        let record = Record {
            attempt,
            step: step.to_string(),
            begun: clock::now_millis(),
            keeper: None,
            child: None,
            ended: None,
        };
        record.write(&self.record())?;
        let lock = Lock::try_take_within(&self.lock(), LOOKED_AT)?.ok_or(LaunchError::Running)?;
        let binary = std::env::current_exe().map_err(LaunchError::NoBinary)?;
        let lock_fd = lock.as_fd().as_raw_fd();
        let mut command = Command::new(binary);
        command
            .arg("keep")
            .arg("--lock")
            .arg(lock_fd.to_string())
            .arg("--record")
            .arg(self.record())
            .arg("--log")
            .arg(self.log.folder())
            .arg("--name")
            .arg(format!("attempt {attempt}, {step}"));
        for (name, _) in &program.env {
            command.arg("--env").arg(name);
        }
        command
            .arg("--")
            .args(&program.argv)
            .env_clear()
            .envs(program.env.iter().map(|(name, value)| (name, value)))
            .current_dir(&program.workdir)
            .stdin(stdin)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: the closure runs in the child between fork and exec, and
        // calls only `setsid` and `fcntl`, which are safe to call there.
        unsafe {
            command.pre_exec(move || {
                // Neither the daemon's session nor a terminal reaches the
                // keeper, and the keeper inherits the lock the daemon holds.
                rustix::process::setsid()?;
                // SAFETY: the lock's file is open in the child as it is in
                // the daemon, which holds it until after the child is
                // started.
                let lock = BorrowedFd::borrow_raw(lock_fd);
                fcntl_setfd(lock, FdFlags::empty())?;
                Ok(())
            });
        }
        let mut keeper = command.spawn().map_err(LaunchError::Spawn)?;
        // The keeper's copy of the lock keeps it held; the daemon's goes.
        drop(lock);
        // The keeper is the daemon's child for as long as the daemon runs,
        // and is reaped when it ends; the system takes it over when the
        // daemon ends first.
        std::thread::spawn(move || keeper.wait());
        Ok(())
    }

    /// Waits until the step under way ends, and gives how its lock and
    /// record say it did: [`Look::Ended`] or [`Look::Lost`].
    pub async fn ended(&self) -> Result<Look, DiskError> {
        loop {
            match self.look()? {
                Look::Running(_) => tokio::time::sleep(LOOK).await,
                look => return Ok(look),
            }
        }
    }

    /// Waits until the keeper of the step under way has started its child,
    /// or the step has ended.
    pub async fn child_started(&self) -> Result<Look, DiskError> {
        loop {
            match self.look()? {
                Look::Running(record) if record.child.is_none() => {
                    tokio::time::sleep(LOOK).await;
                }
                look => return Ok(look),
            }
        }
    }
}
