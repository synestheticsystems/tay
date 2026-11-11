//! Writing a file unit's file: whole and then moved into place, so no
//! reader sees half a file; again only when its content would differ; and
//! never again when `overwrite` is false and it exists. Such a file belongs
//! to the program that rewrites it from then on, which may take it as its
//! own: tay never reads it again, and a unit that uses it was started from
//! its being there, not from its content.

use std::fs::{self, Permissions};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::{DIR, FileSettings, PATH};
use crate::disk::{DiskError, write_whole};
use crate::module::operations::{
    self, Exists, Failure, Made, Taking, UnitPlace, Values, WrongType,
};
use crate::value::Filled;

/// A file's mode when `mode` does not say, and when its content holds a
/// secret.
const MODE: u32 = 0o644;
const SECRET_MODE: u32 = 0o600;

#[derive(Debug, thiserror::Error)]
enum FileError {
    #[error("`mode` is `{0}`, which is not permissions in octal")]
    NotAMode(String),
    #[error("`{}` does not exist", .0.display())]
    Missing(PathBuf),
    #[error("`{}` does not hold what it would be written with", .0.display())]
    Differs(PathBuf),
    #[error("`{}` has the mode {found:04o}, and {wanted:04o} is wanted", path.display())]
    Mode {
        path: PathBuf,
        found: u32,
        wanted: u32,
    },
    #[error(transparent)]
    Disk(#[from] DiskError),
    #[error(transparent)]
    WrongType(#[from] WrongType),
}

/// What the unit's file is to be.
struct Wanted {
    path: PathBuf,
    /// The content, which may hold a secret: it is never shown.
    content: String,
    mode: u32,
    overwrite: bool,
}

impl Made for FileSettings<Filled> {
    fn make(&self, place: &UnitPlace, _: bool) -> Result<Exists, Failure> {
        let wanted = self.wanted(place)?;
        let detail = match on_disk(&wanted)? {
            OnDisk::Missing => {
                let folder = wanted
                    .path
                    .parent()
                    .expect("a unit's file is in its folder");
                fs::create_dir_all(folder).map_err(DiskError::io(folder, "made"))?;
                write(&wanted)?;
                "written"
            }
            OnDisk::Left => "left as it is, since `overwrite` is false",
            OnDisk::Read { content, .. } if content != wanted.content.as_bytes() => {
                write(&wanted)?;
                "written again"
            }
            OnDisk::Read { mode, .. } if mode != wanted.mode => {
                fs::set_permissions(&wanted.path, Permissions::from_mode(wanted.mode))
                    .map_err(DiskError::io(&wanted.path, "given its mode"))?;
                "given its mode"
            }
            OnDisk::Read { .. } => "as written before",
        };
        Ok(exists(&wanted, detail))
    }

    fn check(&self, place: &UnitPlace) -> Result<Exists, Failure> {
        let wanted = self.wanted(place)?;
        match on_disk(&wanted)? {
            OnDisk::Missing => Err(FileError::Missing(wanted.path).into()),
            OnDisk::Left => Ok(exists(&wanted, "written")),
            OnDisk::Read { content, .. } if content != wanted.content.as_bytes() => {
                Err(FileError::Differs(wanted.path).into())
            }
            OnDisk::Read { mode, .. } if mode != wanted.mode => Err(FileError::Mode {
                path: wanted.path,
                found: mode,
                wanted: wanted.mode,
            }
            .into()),
            OnDisk::Read { .. } => Ok(exists(&wanted, "written")),
        }
    }

    fn take_written(&self, taking: &mut Taking, name: &str, path: &Path) -> Result<(), Failure> {
        match self.overwrite()? {
            true => taking.file(name, path)?,
            false => taking.presence(name, path)?,
        }
        Ok(())
    }
}

impl FileSettings<Filled> {
    fn wanted(&self, place: &UnitPlace) -> Result<Wanted, FileError> {
        let mode = match &self.mode {
            Some(mode) => {
                let mode = operations::text(mode, "mode")?;
                u32::from_str_radix(mode, 8).map_err(|_| FileError::NotAMode(mode.to_string()))?
            }
            None if self.content.is_secret() => SECRET_MODE,
            None => MODE,
        };
        Ok(Wanted {
            path: self.path(place),
            content: self.content.reveal(),
            mode,
            overwrite: self.overwrite()?,
        })
    }

    /// Whether the file is written again whenever its content would differ.
    fn overwrite(&self) -> Result<bool, WrongType> {
        match &self.overwrite {
            Some(overwrite) => operations::boolean(overwrite, "overwrite"),
            None => Ok(true),
        }
    }
}

/// The unit's file as tay finds it.
enum OnDisk {
    Missing,
    /// There, and not read: once written, a file tay does not write again
    /// is the program's that rewrites it.
    Left,
    Read {
        content: Vec<u8>,
        mode: u32,
    },
}

/// What is at the path of the file `wanted` describes.
fn on_disk(wanted: &Wanted) -> Result<OnDisk, FileError> {
    let path = &wanted.path;
    let missing = |error: &io::Error| error.kind() == io::ErrorKind::NotFound;
    if !wanted.overwrite {
        return match fs::symlink_metadata(path) {
            Ok(_) => Ok(OnDisk::Left),
            Err(error) if missing(&error) => Ok(OnDisk::Missing),
            Err(error) => Err(DiskError::io(path, "looked at")(error).into()),
        };
    }
    match fs::read(path) {
        Ok(content) => {
            let metadata = fs::metadata(path).map_err(DiskError::io(path, "read"))?;
            let mode = metadata.permissions().mode() & 0o7777;
            Ok(OnDisk::Read { content, mode })
        }
        Err(error) if missing(&error) => Ok(OnDisk::Missing),
        Err(error) => Err(DiskError::io(path, "read")(error).into()),
    }
}

fn write(wanted: &Wanted) -> Result<(), FileError> {
    Ok(write_whole(
        &wanted.path,
        wanted.content.as_bytes(),
        wanted.mode,
    )?)
}

fn exists(wanted: &Wanted, detail: &str) -> Exists {
    let path = &wanted.path;
    let dir = path.parent().expect("a unit's file is in its folder");
    Exists {
        values: Values::from([
            (PATH.to_string(), Filled::text(path.display().to_string())),
            (DIR.to_string(), Filled::text(dir.display().to_string())),
        ]),
        detail: detail.to_string(),
    }
}
