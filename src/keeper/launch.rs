//! The daemon's side of a keeper: starting one for a step of a unit under
//! the unit's lock, looking at the lock and the record to find a step again,
//! waiting for a step to end, and stopping it.
//!
//! The order of a start closes every gap: the daemon writes the step to the
//! unit's record, takes the unit's lock, and starts the keeper, which
//! inherits the lock. The lock is held without a gap from before the keeper
//! exists until it ends, and the system releases it if the keeper dies.

use std::collections::HashMap;
use std::fs::{DirBuilder, File};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rustix::io::{FdFlags, fcntl_setfd};

use super::group::{GRACE, Group};
use super::record::{Ended, Process, Record};
use crate::clock;
use crate::disk::{DiskError, Layout, Lock};
use crate::unit_log::UnitLog;

/// How often a daemon looks at a step under way for its end.
const LOOK: Duration = Duration::from_millis(250);

/// How long a keeper has to write its child once it is started, before a
/// step that is stopped is taken to have none.
const WRITES_CHILD: Duration = Duration::from_secs(2);

/// How long a keeper has to end once its child's group is gone.
const ENDS: Duration = Duration::from_secs(3);

/// What one unit that runs on the host keeps: in `state/units/<unit>/`, its
/// lock and its record, readable by this account only; in `logs/<unit>/`,
/// its log.
#[derive(Clone, Debug)]
pub struct UnitFiles {
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
    #[error("processes of its group {group} were left after they were killed")]
    GroupStays { group: i32 },
    #[error("its processes cannot be signalled: {0}")]
    Signal(rustix::io::Errno),
    #[error(transparent)]
    Disk(#[from] DiskError),
}

impl UnitFiles {
    pub fn of(layout: &Layout, unit: &str) -> UnitFiles {
        UnitFiles {
            folder: units(layout).join(unit),
            log: UnitLog::at(layout.unit_log(unit)),
        }
    }

    /// Whether anything of any unit of the environment `layout` lays out
    /// runs, from what each unit's lock and record say: what a command asks
    /// before it says nothing runs where no daemon does.
    pub fn anything_runs(layout: &Layout) -> Result<bool, DiskError> {
        let folder = units(layout);
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(DiskError::io(folder, "read")(error)),
        };
        for entry in entries {
            let entry = entry.map_err(DiskError::io(&folder, "read"))?;
            let Some(unit) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if UnitFiles::of(layout, &unit).anything_left()? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The unit's own folder in the state folder, where what it produces is
    /// kept beside its lock and record.
    pub fn folder(&self) -> &PathBuf {
        &self.folder
    }

    fn lock(&self) -> PathBuf {
        self.folder.join("lock")
    }

    fn record(&self) -> PathBuf {
        self.folder.join("record.json")
    }

    /// Makes the unit's folder, for this account alone.
    pub fn make(&self) -> Result<(), DiskError> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.folder)
            .map_err(DiskError::io(&self.folder, "made"))
    }

    /// The unit's step as its lock and its record say now.
    pub fn look(&self) -> Result<Look, DiskError> {
        if !self.folder.exists() {
            return Ok(Look::Never);
        }
        // Taking the lock to look at it holds it no longer than this.
        let held = Lock::try_take(&self.lock())?.is_none();
        let record = Record::read(&self.record())?;
        Ok(match (held, record) {
            (true, Some(record)) => {
                if let Some(child) = record.child {
                    remember(&self.folder, child);
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
        // lock is tried, and let go, before the record is written.
        if Lock::try_take(&self.lock())?.is_none() {
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
        let lock = Lock::try_take(&self.lock())?.ok_or(LaunchError::Running)?;
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
            .arg(format!("attempt {attempt}, {step}"))
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

    /// Stops what of the unit runs: the step under way, by its child's
    /// group, and what is left of the group of a step whose keeper was lost,
    /// or whose record has gone with the environment's folder. Waits until
    /// the keeper has ended and nothing of the group is left. It blocks, for
    /// as long as the group takes to end.
    pub fn stop(&self) -> Result<(), LaunchError> {
        let child = match self.look()? {
            Look::Ended(..) => None,
            Look::Never => seen(&self.folder),
            Look::Lost(record) => record.child,
            Look::Running(record) => match record.child {
                Some(child) => Some(child),
                None => self.wait_for_child()?,
            },
        };
        if let Some(child) = child
            && !Group::led_by(child).stop().map_err(LaunchError::Signal)?
        {
            return Err(LaunchError::GroupStays { group: child.pid });
        }
        self.keeper_ended()?;
        forget(&self.folder);
        Ok(())
    }

    /// The child of the step under way once its keeper has written it,
    /// waited for a while; `None` when the step ends meanwhile, or its keeper
    /// writes none.
    fn wait_for_child(&self) -> Result<Option<Process>, LaunchError> {
        let until = Instant::now() + WRITES_CHILD;
        loop {
            match self.look()? {
                Look::Running(Record {
                    child: Some(child), ..
                })
                | Look::Lost(Record {
                    child: Some(child), ..
                }) => return Ok(Some(child)),
                Look::Running(_) if Instant::now() < until => std::thread::sleep(LOOK),
                _ => return Ok(None),
            }
        }
    }

    /// Waits until the unit's lock is free: its keeper has ended.
    fn keeper_ended(&self) -> Result<(), LaunchError> {
        let until = Instant::now() + ENDS + GRACE;
        while matches!(self.look()?, Look::Running(_)) {
            if Instant::now() >= until {
                return Err(LaunchError::KeeperStays);
            }
            std::thread::sleep(LOOK);
        }
        Ok(())
    }
}

/// `state/units/`: a folder for each unit that runs on the host.
fn units(layout: &Layout) -> PathBuf {
    layout.state().join("units")
}

/// The child of each unit's step under way, as this daemon last saw it in
/// the unit's record, by the unit's folder: what it stops when the
/// environment's folder, and the records in it, have gone while the stack
/// runs.
fn children() -> &'static Mutex<HashMap<PathBuf, Process>> {
    static CHILDREN: OnceLock<Mutex<HashMap<PathBuf, Process>>> = OnceLock::new();
    CHILDREN.get_or_init(Mutex::default)
}

fn remember(folder: &Path, child: Process) {
    children()
        .lock()
        .expect("the children seen are not poisoned")
        .insert(folder.to_path_buf(), child);
}

fn seen(folder: &Path) -> Option<Process> {
    children()
        .lock()
        .expect("the children seen are not poisoned")
        .get(folder)
        .copied()
}

fn forget(folder: &Path) {
    children()
        .lock()
        .expect("the children seen are not poisoned")
        .remove(folder);
}
