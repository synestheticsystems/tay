//! Where an environment's daemon listens, as the daemon writes it in
//! `.tay/daemon.socket`. The socket is in a short folder in the runtime
//! folder of the session the daemon was started in, and sessions disagree
//! on where that is: systemd gives a service no `XDG_RUNTIME_DIR`, while a
//! person's login has one. So the daemon writes the path once it listens,
//! and a command finds the daemon by what is written, whatever variables
//! either was started with.

use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::Layout;
use crate::disk::{DiskError, remove_if_there, socket_fits, write_whole};

/// The socket the daemon of the environment at `layout` wrote that it
/// listens on; `None` when nothing is written. A daemon that died leaves
/// it written, so only an answer on it shows that a daemon listens there.
pub fn read(layout: &Layout) -> Result<Option<PathBuf>, DiskError> {
    let path = layout.daemon_socket();
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DiskError::io(path, "read")(error)),
    };
    let socket = PathBuf::from(OsStr::from_bytes(
        bytes.strip_suffix(b"\n").unwrap_or(&bytes),
    ));
    if !socket.is_absolute() || !socket_fits(&socket) {
        return Err(DiskError::Unexpected {
            path,
            found: socket.display().to_string(),
            wanted: "the path of a daemon's socket",
        });
    }
    Ok(Some(socket))
}

/// Writes that the daemon of the environment at `layout` listens on
/// `socket`, whole, so that no command reads half of it.
pub fn write(layout: &Layout, socket: &Path) -> Result<(), DiskError> {
    let mut bytes = socket.as_os_str().as_bytes().to_vec();
    bytes.push(b'\n');
    write_whole(&layout.daemon_socket(), &bytes, 0o644)
}

/// Forgets that the daemon listens on `socket`, if that is what is
/// written: once the daemon's lock has been taken from under it, as
/// `tay wipe --all` does, another daemon may have written its own.
pub fn forget(layout: &Layout, socket: &Path) -> Result<(), DiskError> {
    match read(layout) {
        Ok(Some(written)) if written == socket => remove_if_there(&layout.daemon_socket()),
        Ok(_) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_written_is_read_back_and_forgotten_only_by_the_daemon_that_wrote_it() {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::of(root.path());
        std::fs::create_dir(layout.folder()).unwrap();
        assert_eq!(read(&layout).unwrap(), None);
        let socket = Path::new("/run/user/1000/tay/0123456789ab/daemon.sock");
        write(&layout, socket).unwrap();
        assert_eq!(read(&layout).unwrap().as_deref(), Some(socket));
        forget(&layout, Path::new("/tmp/tay-1000/0123456789ab/daemon.sock")).unwrap();
        assert_eq!(read(&layout).unwrap().as_deref(), Some(socket));
        forget(&layout, socket).unwrap();
        assert_eq!(read(&layout).unwrap(), None);
    }

    #[test]
    fn a_path_no_daemon_could_have_written_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::of(root.path());
        std::fs::create_dir(layout.folder()).unwrap();
        for written in [
            "daemon.sock\n".to_string(),
            format!("/{}\n", "a".repeat(200)),
        ] {
            std::fs::write(layout.daemon_socket(), written).unwrap();
            assert!(matches!(read(&layout), Err(DiskError::Unexpected { .. })));
        }
    }
}
