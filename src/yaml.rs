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

use crate::source::Loc;

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
    /// The column of the string's first character when every character of the
    /// value sits in the file one after another on one line (no escapes, no
    /// folding), so an offset into the value is an offset in the file.
    pub content_column: Option<u32>,
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
