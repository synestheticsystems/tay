//! Finding the stack a command acts on: the nearest folder at or above the
//! current one, or the one `--root` names, that holds `tay.yaml`.

use std::path::{Path, PathBuf};

use crate::definition::read::{self, STACK_FILE};

/// The stack's root, from `--root` when it is given; why there is none.
pub fn root(asked: Option<&Path>) -> Result<PathBuf, String> {
    let start = match asked {
        Some(asked) => asked.to_path_buf(),
        None => std::env::current_dir()
            .map_err(|error| format!("the current folder cannot be read: {error}"))?,
    };
    let start = start
        .canonicalize()
        .map_err(|error| format!("the folder `{}` cannot be used: {error}", start.display()))?;
    read::find_root(&start).ok_or_else(|| {
        format!(
            "there is no {STACK_FILE} in {} or any folder above it",
            start.display()
        )
    })
}
