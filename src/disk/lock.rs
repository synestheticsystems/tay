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
        loop {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)
                .map_err(DiskError::io(path, "opened to be locked"))?;
            match Lock::on(file, path)? {
                Taking::Taken(lock) => return Ok(Some(lock)),
                Taking::Held => return Ok(None),
                // Its holder removed the file before it let go of it, as
                // one that removes what an environment keeps does: the lock
                // is had on the file at `path` now, made again.
                Taking::Removed => {}
            }
        }
    }

    /// The lock on `file`, opened at `path`, unless another holds it or the
    /// file is no longer the one at `path`.
    fn on(file: File, path: &Path) -> Result<Taking, DiskError> {
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {
                let lock = Lock { file };
                Ok(match lock.is_at(path) {
                    true => Taking::Taken(lock),
                    false => Taking::Removed,
                })
            }
            Err(Errno::WOULDBLOCK) => Ok(Taking::Held),
            Err(errno) => Err(DiskError::io(path, "locked")(errno.into())),
        }
    }

    /// The lock on `path`, tried again while another holds it, for as long
    /// as `limit`; `None` if another holds it still.
    pub fn try_take_within(path: &Path, limit: Duration) -> Result<Option<Lock>, DiskError> {
        let until = Instant::now() + limit;
        loop {
            if let Some(lock) = Lock::try_take(path)? {
                return Ok(Some(lock));
            }
            if Instant::now() >= until {
                return Ok(None);
            }
            std::thread::sleep(RETRY);
        }
    }

    /// Whether another holds the lock on `path`. Looking takes a shared
    /// lock for a moment, which other looks share, so that looking keeps
    /// the lock from another only for that moment.
    pub fn is_held(path: &Path) -> Result<bool, DiskError> {
        let file = match OpenOptions::new().read(true).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(DiskError::io(path, "opened to be looked at")(error)),
        };
        match flock(&file, FlockOperation::NonBlockingLockShared) {
            Ok(()) => Ok(false),
            Err(Errno::WOULDBLOCK) => Ok(true),
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

/// What came of trying to take a lock on a file that was opened.
enum Taking {
    Taken(Lock),
    /// Another holds it.
    Held,
    /// The file is no longer the one at its path.
    Removed,
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
        // A process started meanwhile by another test holds every open file
        // of this one until it runs its program, so the lock may stay held
        // for that moment.
        let again = Lock::try_take_within(&path, Duration::from_secs(1))
            .unwrap()
            .expect("the lock is free again");
        std::fs::remove_file(&path).unwrap();
        assert!(!again.is_at(&path));
    }

    #[test]
    fn a_lock_on_a_file_removed_before_its_holder_let_go_is_taken_on_the_file_made_again() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("a.lock");
        let held = Lock::try_take(&path).unwrap().expect("the lock is free");
        // One that opened the file before its holder removed it, and locks
        // it after the holder has let go.
        let opened = File::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        drop(held);
        let until = Instant::now() + Duration::from_secs(1);
        let taking = loop {
            match Lock::on(opened.try_clone().unwrap(), &path).unwrap() {
                // As above, another test's process may hold it a moment.
                Taking::Held if Instant::now() < until => std::thread::sleep(RETRY),
                taking => break taking,
            }
        };
        assert!(matches!(taking, Taking::Removed));
        let again = Lock::try_take(&path).unwrap().expect("the lock is free");
        assert!(again.is_at(&path));
    }

    #[test]
    fn looking_at_a_lock_says_whether_another_holds_it_and_keeps_it_from_none() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("a.lock");
        assert!(!Lock::is_held(&path).unwrap(), "no file, no holder");
        let held = Lock::try_take(&path).unwrap().unwrap();
        assert!(Lock::is_held(&path).unwrap());
        drop(held);
        // A look holds nothing once it has looked.
        assert!(
            Lock::try_take_within(&path, Duration::from_secs(1))
                .unwrap()
                .is_some()
        );
    }
}
