//! What git ignores, asked of git itself, so that a file or folder a
//! repository ignores is left out of what is watched as git leaves it out.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::AsyncWriteExt;

/// Of `paths`, those git does not ignore, asked of git in `folder`: every
/// one of them where `folder` is in no repository of git's.
pub async fn not_ignored(folder: &Path, paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let git = || {
        let mut git = tokio::process::Command::new("git");
        git.arg("-C").arg(folder).stderr(Stdio::piped());
        git
    };
    let inside = git()
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(Stdio::piped())
        .output()
        .await
        .map_err(|error| format!("git cannot be run: {error}"))?;
    if !inside.status.success() {
        return Ok(paths);
    }
    let mut asking = git()
        .args(["check-ignore", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("git cannot be run: {error}"))?;
    let mut asked = Vec::new();
    for path in &paths {
        asked.extend_from_slice(path.as_os_str().as_encoded_bytes());
        asked.push(0);
    }
    let mut stdin = asking.stdin.take().expect("its standard input is piped");
    stdin
        .write_all(&asked)
        .await
        .map_err(|error| format!("git cannot be asked: {error}"))?;
    drop(stdin);
    let answer = asking
        .wait_with_output()
        .await
        .map_err(|error| format!("git cannot be asked: {error}"))?;
    // It exits with 1 when it ignores none of them.
    if !matches!(answer.status.code(), Some(0 | 1)) {
        return Err(String::from_utf8_lossy(&answer.stderr).trim().to_string());
    }
    let ignored: BTreeSet<&[u8]> = answer
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .collect();
    Ok(paths
        .into_iter()
        .filter(|path| !ignored.contains(path.as_os_str().as_encoded_bytes()))
        .collect())
}
