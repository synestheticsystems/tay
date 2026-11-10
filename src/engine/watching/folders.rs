//! Which folders the system is asked to watch. FSEvents, on macOS, watches
//! a folder with everything under it as one stream, which costs the same
//! however much is under it; what git ignores is left out of what it says
//! changed. inotify, on Linux, watches one folder at a time, and each watch
//! counts against a limit the system sets for every program of the
//! account: so there a folder git ignores, a build folder's thousands with
//! it, is not watched at all, and a folder made later under a watched one
//! is watched once it is seen, unless git ignores it.

use std::path::{Path, PathBuf};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Watches `path`, a file or a folder a unit's `watch` names, and
/// everything under a folder that git does not ignore.
#[cfg(not(target_os = "linux"))]
pub async fn watch(watcher: &mut RecommendedWatcher, path: &Path) -> Result<(), String> {
    added(watcher.watch(path, RecursiveMode::Recursive), path)
}

/// Watches each folder of `appeared`, which were made or moved under
/// `folder`, a folder a unit's `watch` names, since it was watched; gives
/// the files in them. Nothing is to be done where the system watches
/// everything under `folder` already.
#[cfg(not(target_os = "linux"))]
pub async fn appeared(
    _: &mut RecommendedWatcher,
    _: &Path,
    _: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    Ok(Vec::new())
}

/// Watches `path`, a file or a folder a unit's `watch` names, and
/// everything under a folder that git does not ignore.
#[cfg(target_os = "linux")]
pub async fn watch(watcher: &mut RecommendedWatcher, path: &Path) -> Result<(), String> {
    match path.is_dir() {
        true => each_folder(watcher, path, vec![path.to_path_buf()])
            .await
            .map(drop),
        false => added(watcher.watch(path, RecursiveMode::NonRecursive), path),
    }
}

/// Watches each folder of `appeared`, which were made or moved under
/// `folder`, a folder a unit's `watch` names, since it was watched, and
/// every folder under them, each that git does not ignore; gives the files
/// in them, which are new to what is watched.
#[cfg(target_os = "linux")]
pub async fn appeared(
    watcher: &mut RecommendedWatcher,
    folder: &Path,
    appeared: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let folders: Vec<PathBuf> = appeared.into_iter().filter(|path| path.is_dir()).collect();
    if folders.is_empty() {
        return Ok(Vec::new());
    }
    let folders = super::ignored::not_ignored(folder, folders).await?;
    each_folder(watcher, folder, folders).await
}

/// Watches each of `level`, folders under `root` that git does not ignore,
/// and every folder under them that git does not ignore either, a level at
/// a time, so that git is asked once for each level; gives the files found
/// in them. What git keeps of a repository is none of its files, and a
/// link to a folder is not followed.
#[cfg(target_os = "linux")]
async fn each_folder(
    watcher: &mut RecommendedWatcher,
    root: &Path,
    mut level: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    while !level.is_empty() {
        let mut below = Vec::new();
        for folder in &level {
            added(watcher.watch(folder, RecursiveMode::NonRecursive), folder)?;
            let unread =
                |error: std::io::Error| format!("`{}` cannot be read: {error}", folder.display());
            let entries = match std::fs::read_dir(folder) {
                Ok(entries) => entries,
                // One removed since it was seen is no longer watched.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(unread(error)),
            };
            for entry in entries {
                let entry = entry.map_err(unread)?;
                let kind = entry.file_type().map_err(unread)?;
                if kind.is_dir() && entry.file_name() != ".git" {
                    below.push(entry.path());
                } else if kind.is_file() {
                    files.push(entry.path());
                }
            }
        }
        level = super::ignored::not_ignored(root, below).await?;
    }
    Ok(files)
}

/// What became of asking the system to watch `path`.
fn added(watched: notify::Result<()>, path: &Path) -> Result<(), String> {
    watched.map_err(|error| format!("`{}` cannot be watched: {error}", path.display()))
}
