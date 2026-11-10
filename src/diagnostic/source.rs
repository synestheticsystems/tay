//! Where a piece of a definition came from, so that every error can point at it.

use std::path::{Path, PathBuf};

/// Identifies one entry in [`Sources`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(u32);

/// A place definitions are read from: a file, or one `--set` argument.
#[derive(Debug)]
pub enum Source {
    /// A file, kept as it is shown (relative to the stack's root when it is
    /// inside it) and as it is on disk.
    File { shown: String, path: PathBuf },
    /// A command-line argument, kept as it is shown in errors: a `--set` is
    /// shown by its key only, since its value may be a secret.
    Argument { shown: String },
}

/// Every source read in one run. Errors refer to sources by [`SourceId`].
#[derive(Debug, Default)]
pub struct Sources {
    list: Vec<Source>,
}

impl Sources {
    /// Registers a file. `root` is the stack's root: files inside it are shown
    /// relative to it, so that error lines are short and the same on every machine.
    pub fn add_file(&mut self, path: &Path, root: &Path) -> SourceId {
        let shown = match path.strip_prefix(root) {
            Ok(relative) => relative.display().to_string(),
            Err(_) => path.display().to_string(),
        };
        self.push(Source::File {
            shown,
            path: path.to_path_buf(),
        })
    }

    pub fn add_argument(&mut self, shown: &str) -> SourceId {
        self.push(Source::Argument {
            shown: shown.to_string(),
        })
    }

    fn push(&mut self, source: Source) -> SourceId {
        let id = SourceId(u32::try_from(self.list.len()).expect("fewer than 2^32 sources"));
        self.list.push(source);
        id
    }

    pub fn get(&self, id: SourceId) -> &Source {
        &self.list[id.0 as usize]
    }
}

/// A position in a source: line and column, both counted from 1, columns in characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Loc {
    pub source: SourceId,
    pub line: u32,
    pub column: u32,
}

impl Loc {
    pub fn new(source: SourceId, line: u32, column: u32) -> Loc {
        Loc {
            source,
            line,
            column,
        }
    }

    /// The same line, `chars` characters further right.
    pub fn shifted(self, chars: usize) -> Loc {
        let chars = u32::try_from(chars).unwrap_or(u32::MAX);
        Loc {
            column: self.column.saturating_add(chars),
            ..self
        }
    }

    /// Writes `file:line:column`, or the argument as it is shown.
    pub fn render(&self, sources: &Sources) -> String {
        match sources.get(self.source) {
            Source::File { shown, .. } => format!("{shown}:{}:{}", self.line, self.column),
            Source::Argument { shown } => shown.clone(),
        }
    }
}
