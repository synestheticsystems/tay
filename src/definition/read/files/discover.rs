//! Finding a stack's root and the definition files in its folders.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The file that marks the root of a stack.
pub const STACK_FILE: &str = "tay.yaml";

/// The nearest folder at or above `start` that holds `tay.yaml`.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|folder| folder.join(STACK_FILE).is_file())
        .map(Path::to_path_buf)
}

/// One entry of a definitions folder.
pub enum Entry {
    /// A `.yaml` file.
    File(PathBuf),
    Folder(PathBuf),
    /// Anything else, which tay does not read and must not skip silently.
    Other(PathBuf),
}

/// The entries of `folder`, sorted by name so that every run reads them in the
/// same order. Entries whose names start with a dot are not definitions.
/// `Ok(None)` when the folder does not exist.
pub fn entries(folder: &Path) -> io::Result<Option<Vec<Entry>>> {
    let listing = match fs::read_dir(folder) {
        Ok(listing) => listing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut paths = Vec::new();
    for entry in listing {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with('.') {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(Some(
        paths
            .into_iter()
            .map(|path| {
                // `is_dir` follows a symbolic link, so a linked folder is read too.
                if path.is_dir() {
                    Entry::Folder(path)
                } else if path.extension().is_some_and(|ext| ext == "yaml") {
                    Entry::File(path)
                } else {
                    Entry::Other(path)
                }
            })
            .collect(),
    ))
}

/// The name of a file or folder without its `.yaml`.
pub fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}
