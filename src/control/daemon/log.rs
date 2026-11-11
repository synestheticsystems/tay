//! The daemon's own log, `.tay/logs/tay/daemon.log`: each line with its
//! time, and a new log begun when it has grown too long, so that an
//! environment brought up for months does not fill the disk. A daemon
//! started detached writes it as its standard error, which the command that
//! starts it makes the log. `tay serve` writes it to a file of its own,
//! since its standard error is the service manager's.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::clock;
use crate::disk::DiskError;
use crate::print;

/// A log longer than this is set aside and a new one begun. One file set
/// aside is kept.
const LOG_SIZE: u64 = 1 << 20;

/// The daemon's log, once the daemon runs.
static LOG: OnceLock<Log> = OnceLock::new();

struct Log {
    path: PathBuf,
    /// The file it is written to, for a daemon `tay serve` runs; `None` for
    /// one started detached, whose standard error is the log.
    own: Option<Mutex<File>>,
}

/// The daemon's log is at `path`, and is its standard error, as the
/// command that starts a daemon detached makes it. It is set once, as the
/// daemon starts.
pub fn on_stderr(path: PathBuf) {
    let _ = LOG.set(Log { path, own: None });
}

/// The daemon's log is the file at `path`, opened here, for a daemon whose
/// standard error is another's. It is set once, as the daemon starts.
pub fn in_own_file(path: PathBuf) -> Result<(), DiskError> {
    let file = open_log(&path)?;
    let _ = LOG.set(Log {
        path,
        own: Some(Mutex::new(file)),
    });
    Ok(())
}

/// The daemon's log at `path`, opened to be added to; a long one is set
/// aside first.
pub fn open_log(path: &Path) -> Result<File, DiskError> {
    let folder = path
        .parent()
        .expect("the daemon's log is in the logs folder");
    std::fs::create_dir_all(folder).map_err(DiskError::io(folder, "made"))?;
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > LOG_SIZE) {
        let set_aside = path.with_extension("log.1");
        std::fs::rename(path, &set_aside).map_err(DiskError::io(path, "set aside"))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(DiskError::io(path, "opened"))
}

/// Writes a line to the daemon's log, with its time, and begins a new log
/// when it has grown too long. Before the log is set, the line goes to
/// standard error.
pub fn log(line: &str) {
    let text = format!("{}  {line}\n", clock::utc(clock::now()));
    let Some(log) = LOG.get() else {
        print::err(&text);
        return;
    };
    match &log.own {
        Some(file) => {
            let mut file = file.lock().expect("the daemon's log is not poisoned");
            // A log that cannot be written has nowhere else to say so.
            let _ = file.write_all(text.as_bytes());
            if let Some(anew) = log.begun_anew(&*file) {
                *file = anew;
            }
        }
        None => {
            print::err(&text);
            if let Some(anew) = log.begun_anew(rustix::stdio::stderr()) {
                // What the daemon says on its standard error, as a panic
                // does, goes on reaching the log.
                let _ = rustix::stdio::dup2_stderr(&anew);
            }
        }
    }
}

impl Log {
    /// A new log, the one `written` is set aside, once that has grown too
    /// long. `None` while it has not, and when a new one cannot be begun,
    /// as when its folder has gone with the environment's, which is not
    /// made again: the log is then written on as it is, since there is
    /// nowhere else to say so.
    fn begun_anew(&self, written: impl AsFd) -> Option<File> {
        let long = rustix::fs::fstat(written)
            .is_ok_and(|stat| u64::try_from(stat.st_size).is_ok_and(|size| size > LOG_SIZE));
        if !long || !self.path.parent().is_some_and(Path::exists) {
            return None;
        }
        open_log(&self.path).ok()
    }
}
