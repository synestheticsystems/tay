//! Writing a file unit's file: whole and then moved into place, so no
//! reader sees half a file; again only when its content would differ; and
//! never again when `overwrite` is false and it exists.

use std::fs::{self, Permissions};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::{DIR, FILES, FileSettings, PATH};
use crate::disk::{DiskError, write_whole};
use crate::module::operations::{self, Exists, Failure, Made, UnitPlace, Values, WrongType};
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
    fn make(&self, place: &UnitPlace<'_>) -> Result<Exists, Failure> {
        let wanted = self.wanted(place)?;
        let detail = match on_disk(&wanted.path)? {
            None => {
                let folder = wanted
                    .path
                    .parent()
                    .expect("a unit's file is in its folder");
                fs::create_dir_all(folder).map_err(DiskError::io(folder, "made"))?;
                write(&wanted)?;
                "written"
            }
            Some(_) if !wanted.overwrite => "left as it is, since `overwrite` is false",
            Some((content, _)) if content != wanted.content.as_bytes() => {
                write(&wanted)?;
                "written again"
            }
            Some((_, mode)) if mode != wanted.mode => {
                fs::set_permissions(&wanted.path, Permissions::from_mode(wanted.mode))
                    .map_err(DiskError::io(&wanted.path, "given its mode"))?;
                "given its mode"
            }
            Some(_) => "as written before",
        };
        Ok(exists(&wanted, detail))
    }

    fn check(&self, place: &UnitPlace<'_>) -> Result<Exists, Failure> {
        let wanted = self.wanted(place)?;
        match on_disk(&wanted.path)? {
            None => Err(FileError::Missing(wanted.path).into()),
            Some(_) if !wanted.overwrite => Ok(exists(&wanted, "written")),
            Some((content, _)) if content != wanted.content.as_bytes() => {
                Err(FileError::Differs(wanted.path).into())
            }
            Some((_, mode)) if mode != wanted.mode => Err(FileError::Mode {
                path: wanted.path,
                found: mode,
                wanted: wanted.mode,
            }
            .into()),
            Some(_) => Ok(exists(&wanted, "written")),
        }
    }
}

impl FileSettings<Filled> {
    fn wanted(&self, place: &UnitPlace<'_>) -> Result<Wanted, FileError> {
        let mode = match &self.mode {
            Some(mode) => {
                let mode = operations::text(mode, "mode")?;
                u32::from_str_radix(mode, 8).map_err(|_| FileError::NotAMode(mode.to_string()))?
            }
            None if self.content.is_secret() => SECRET_MODE,
            None => MODE,
        };
        let overwrite = match &self.overwrite {
            Some(overwrite) => operations::boolean(overwrite, "overwrite")?,
            None => true,
        };
        let path = place
            .environment
            .layout
            .state()
            .join(FILES)
            .join(&self.folder)
            .join(&self.name.name);
        Ok(Wanted {
            path,
            content: self.content.reveal(),
            mode,
            overwrite,
        })
    }
}

/// The file's content and mode, when there is one at `path`.
fn on_disk(path: &Path) -> Result<Option<(Vec<u8>, u32)>, FileError> {
    match fs::read(path) {
        Ok(content) => {
            let metadata = fs::metadata(path).map_err(DiskError::io(path, "read"))?;
            Ok(Some((content, metadata.permissions().mode() & 0o7777)))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
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
