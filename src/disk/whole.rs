//! Writing a file whole: into a new file beside it, then moved into place,
//! so that whoever reads it sees the old file or the new one and never half
//! of either; and a secret so, for this account alone.

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::DiskError;

/// Tells apart the new files one process writes at once.
static WRITTEN: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` to `path` whole, with the permissions `mode`. The new
/// file has those permissions from the moment it exists, so a file for its
/// owner alone is never readable by others, even half written.
pub fn write_whole(path: &Path, bytes: &[u8], mode: u32) -> Result<(), DiskError> {
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(DiskError::io(path, "written")(std::io::Error::other(
            "it is not a file's path",
        )));
    };
    let new = folder.join(format!(
        ".{}.tay-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        WRITTEN.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&new)?;
        // The mode a file is made with loses what the umask takes away.
        file.set_permissions(Permissions::from_mode(mode))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&new, path)
    })();
    if written.is_err() {
        // The new file is tay's own and only half written; the old one, if
        // any, is untouched.
        let _ = fs::remove_file(&new);
    }
    written.map_err(DiskError::io(path, "written"))
}

/// Writes `bytes` to `path`, a file in a folder of secrets, for this
/// account alone: the folder is made if it is not there and closed to
/// others, and the file is written whole for its owner alone.
pub fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), DiskError> {
    let Some(folder) = path.parent() else {
        return Err(DiskError::io(path, "written")(std::io::Error::other(
            "it is not a file's path",
        )));
    };
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
        .and_then(|()| fs::set_permissions(folder, Permissions::from_mode(0o700)))
        .map_err(DiskError::io(folder, "made"))?;
    write_whole(path, bytes, 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_replaced_whole_with_its_mode_and_nothing_is_left_beside_it() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("routes.yaml");
        write_whole(&path, b"first", 0o644).unwrap();
        write_whole(&path, b"second", 0o600).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let names: Vec<_> = fs::read_dir(folder.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["routes.yaml"]);
    }
}
