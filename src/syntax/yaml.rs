//! YAML read into a tree in which every node, key or value, knows its position.
//!
//! The tree is built from saphyr-parser's events rather than taken from a
//! loader, for three reasons: a duplicate key must be reported with both of its
//! places and reading must go on after it; a quoted `"5432"` must stay text
//! while a plain `5432` is a number; and a reference inside a string should be
//! reported at its own column, which needs to know whether the string's
//! characters sit in the file exactly as they are in the value.

mod build;
mod schema;

pub use build::parse;

use crate::diagnostic::source::Loc;

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub loc: Loc,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Map(Vec<Entry>),
    Seq(Vec<Node>),
    Scalar(Scalar),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: Node,
    pub value: Node,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Scalar {
    Null,
    Bool(bool),
    Int(i64),
    /// A number with a fraction or an exponent. tay has no use for one, but
    /// knowing that it is one gives a better message than calling it text.
    Float,
    /// A whole number too large for 64 bits.
    TooLarge,
    Str(Str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Str {
    pub text: String,
    /// Where the value's characters were written, which places each
    /// reference in it.
    pub placement: Placement,
}

/// Where the characters of a string's value were written, so that each
/// reference in it can be placed where it was written.
#[derive(Clone, Debug, PartialEq)]
pub enum Placement {
    /// Every character of the value sits in the file one after another on
    /// one line from this column (no escapes, no folding), so an offset into
    /// the value is an offset in the file.
    Column(u32),
    /// Each reference in the value, in order, where it was written: the
    /// value was made by putting a template's parameters in place.
    References(Vec<Loc>),
    /// Not known: every reference is placed at the string's start.
    AtStart,
}

impl Node {
    pub fn as_str(&self) -> Option<&Str> {
        match &self.kind {
            Kind::Scalar(Scalar::Str(s)) => Some(s),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&[Entry]> {
        match &self.kind {
            Kind::Map(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn as_seq(&self) -> Option<&[Node]> {
        match &self.kind {
            Kind::Seq(items) => Some(items),
            _ => None,
        }
    }

    /// Whether nothing is written here: a file that holds only comments,
    /// or a key with nothing, or `null`, after it. Every line under the key
    /// may have been commented out.
    pub fn nothing_written(&self) -> bool {
        matches!(self.kind, Kind::Scalar(Scalar::Null))
    }

    /// What kind of thing this is, for messages: "a list", "text", ...
    pub fn describe(&self) -> &'static str {
        match &self.kind {
            Kind::Map(_) => "a mapping",
            Kind::Seq(_) => "a list",
            Kind::Scalar(Scalar::Null) => "empty",
            Kind::Scalar(Scalar::Bool(_)) => "true or false",
            Kind::Scalar(Scalar::Int(_)) => "a whole number",
            Kind::Scalar(Scalar::Float) => "a number with a fraction",
            Kind::Scalar(Scalar::TooLarge) => "a number too large to hold",
            Kind::Scalar(Scalar::Str(_)) => "text",
        }
    }

    /// `over` laid on this mapping: each of its entries replaces the entry
    /// with the same key, or is added after the rest. An entry whose key
    /// `by_entry` names is itself joined entry by entry when its value is a
    /// mapping on both sides. When either side is not a mapping, the result
    /// is `over`.
    pub fn joined(&self, over: &Node, by_entry: &[&str]) -> Node {
        let (Some(base), Some(top)) = (self.as_map(), over.as_map()) else {
            return over.clone();
        };
        let mut entries = base.to_vec();
        for entry in top {
            let key = entry.key.key_text();
            let existing = entries
                .iter_mut()
                .find(|existing| key.is_some() && existing.key.key_text() == key);
            match existing {
                Some(existing) => {
                    let deep = key.as_deref().is_some_and(|key| by_entry.contains(&key));
                    existing.value = if deep {
                        existing.value.joined(&entry.value, &[])
                    } else {
                        entry.value.clone()
                    };
                    existing.key = entry.key.clone();
                }
                None => entries.push(entry.clone()),
            }
        }
        Node {
            loc: over.loc,
            kind: Kind::Map(entries),
        }
    }

    /// The text a key is compared by when looking for duplicates.
    fn key_text(&self) -> Option<String> {
        match &self.kind {
            Kind::Scalar(Scalar::Str(s)) => Some(s.text.clone()),
            Kind::Scalar(Scalar::Int(n)) => Some(n.to_string()),
            Kind::Scalar(Scalar::Bool(b)) => Some(b.to_string()),
            _ => None,
        }
    }
}
