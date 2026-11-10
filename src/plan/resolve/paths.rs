//! Paths on the host, as tay uses them: one that begins `./` is relative to
//! the folder of the file it is written in, any other relative one to the
//! stack's root.

use std::path::{Component, Path, PathBuf};

use crate::diagnostic::source::Loc;
use crate::plan::resolve::value::Resolved;
use crate::value::{Bit, Value};

use super::Resolver;

impl Resolver<'_> {
    /// The whole path a value written at `written` names. A value whose
    /// start is not known before start (a produced path) is left as it is.
    pub fn host_path(&self, mut value: Resolved, written: Loc) -> Resolved {
        let Value::Text(bits) = &mut value.value else {
            return value;
        };
        let Some(Bit::Text(first)) = bits.first_mut() else {
            return value;
        };
        if first.starts_with('/') {
            return value;
        }
        let (base, rest) = match first.strip_prefix("./") {
            Some(rest) => {
                let folder = self.folders.get(written.source.index()).cloned().flatten();
                (
                    folder.unwrap_or_else(|| PathBuf::from(&self.environment.root)),
                    rest,
                )
            }
            None => (PathBuf::from(&self.environment.root), first.as_str()),
        };
        *first = if rest.is_empty() {
            format!("{}/", base.display())
        } else {
            base.join(rest).display().to_string()
        };
        value
    }

    /// Whether `path` lies in the environment's state folder or in the
    /// shared folder, which tay makes when they are missing.
    pub fn in_own_folders(&self, path: &Path) -> bool {
        normalized(path).starts_with(&self.environment.state) || self.in_shared_folder(path)
    }

    /// Whether `path` lies in the folder all of one person's environments of
    /// this stack share.
    pub fn in_shared_folder(&self, path: &Path) -> bool {
        let path = normalized(path);
        self.environment
            .shared
            .as_ref()
            .is_some_and(|shared| path.starts_with(shared))
    }
}

/// `path` with `.` and `..` worked out, without asking the file system.
pub fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_are_worked_out_lexically() {
        assert_eq!(
            normalized(Path::new("/s/root/./a/../b")),
            PathBuf::from("/s/root/b")
        );
        assert!(!normalized(Path::new("/s/root/../etc")).starts_with("/s/root"));
    }
}
