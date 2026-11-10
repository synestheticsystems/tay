//! Locks on files. A lock is held for as long as its file stays open, and
//! the system releases it when its holder ends however it ends, so a lock
//! says truly whether its holder is alive, where a file of process numbers
//! would not.

use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;

use super::DiskError;

/// How long tay waits for a lock that another tay holds.
pub const WAIT: Duration = Duration::from_secs(30);

/// How often a lock that another holds is tried again.
const RETRY: Duration = Duration::from_millis(20);

/// An exclusive lock on a file, held until it is dropped or its holder ends.
#[derive(Debug)]
pub struct Lock {
    file: File,
}

impl Lock {
    /// The lock on `path`, whose file is made if it is missing; `None` while
    /// another holds it.
    pub fn try_take(path: &Path) -> Result<Option<Lock>, DiskError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(DiskError::io(path, "opened to be locked"))?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Some(Lock { file })),
            Err(Errno::WOULDBLOCK) => Ok(None),
            Err(errno) => Err(DiskError::io(path, "locked")(errno.into())),
        }
    }

    /// The lock on `path`, waited for while another holds it, for as long
    /// as [`WAIT`].
    pub fn take(path: &Path) -> Result<Lock, DiskError> {
        let until = Instant::now() + WAIT;
        loop {
            if let Some(lock) = Lock::try_take(path)? {
                return Ok(lock);
            }
            if Instant::now() >= until {
                return Err(DiskError::StayedLocked {
                    path: path.to_path_buf(),
                });
            }
            std::thread::sleep(RETRY);
        }
    }

    /// Whether the file this lock is held on is still the one at `path`:
    /// not removed, and not replaced by another.
    pub fn is_at(&self, path: &Path) -> bool {
        let (Ok(held), Ok(there)) = (self.file.metadata(), std::fs::metadata(path)) else {
            return false;
        };
        (held.dev(), held.ino()) == (there.dev(), there.ino())
    }
}

/// The lock's open file, which a process started with it inherits, and with
/// it the lock.
impl AsFd for Lock {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_is_had_by_one_holder_at_a_time_and_freed_when_dropped() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("a.lock");
        let held = Lock::try_take(&path).unwrap().expect("the lock is free");
        assert!(Lock::try_take(&path).unwrap().is_none());
        assert!(held.is_at(&path));
        drop(held);
        let again = Lock::try_take(&path)
            .unwrap()
            .expect("the lock is free again");
        std::fs::remove_file(&path).unwrap();
        assert!(!again.is_at(&path));
    }
}
