//! Errors found in definitions, each tied to the place that is wrong.
//!
//! tay reports every error it can find in one run, so errors are collected
//! rather than returned one at a time.

pub mod source;

use source::{Loc, Sources};

/// One error: where it is, what is wrong, and the other places involved.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diagnostic {
    pub loc: Loc,
    pub message: String,
    pub related: Vec<Related>,
}

/// Another place an error involves: the first definition of a duplicate, the
/// declaration of a setting, the other end of a circle.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Related {
    pub loc: Loc,
    pub message: String,
}

impl Diagnostic {
    pub fn new(loc: Loc, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            loc,
            message: message.into(),
            related: Vec::new(),
        }
    }

    pub fn with(mut self, loc: Loc, message: impl Into<String>) -> Diagnostic {
        self.related.push(Related {
            loc,
            message: message.into(),
        });
        self
    }

    /// The error as a person reads it: `file:line:column: what is wrong`, then
    /// one indented line for each other place involved.
    pub fn render(&self, sources: &Sources) -> String {
        let mut text = format!("{}: {}", self.loc.render(sources), self.message);
        for related in &self.related {
            text.push_str(&format!(
                "\n  {}: {}",
                related.loc.render(sources),
                related.message
            ));
        }
        text
    }
}

/// The errors of one run.
#[derive(Debug, Default)]
pub struct Diagnostics {
    list: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.list.push(diagnostic);
    }

    pub fn error(&mut self, loc: Loc, message: impl Into<String>) {
        self.push(Diagnostic::new(loc, message));
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn extend(&mut self, other: Diagnostics) {
        self.list.extend(other.list);
    }

    /// The errors in the order of the sources they are in, each once. The same
    /// error can be found along two paths (a value resolved for two units, a
    /// template's text read for two units made from it), and a person should
    /// see it once, with every other place each path involved.
    pub fn into_sorted(mut self) -> Vec<Diagnostic> {
        self.list.sort();
        let mut merged: Vec<Diagnostic> = Vec::new();
        for diagnostic in self.list {
            match merged.last_mut() {
                Some(last) if last.loc == diagnostic.loc && last.message == diagnostic.message => {
                    for related in diagnostic.related {
                        if !last.related.contains(&related) {
                            last.related.push(related);
                        }
                    }
                }
                _ => merged.push(diagnostic),
            }
        }
        merged
    }
}
