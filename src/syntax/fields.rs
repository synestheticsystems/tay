//! Reading a mapping strictly: each field taken by name, and any key that is
//! not a field of the thing being read reported as an error.

use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Loc;
use crate::syntax::expr::Literal;
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::{Kind, Node, Scalar};

pub struct Fields<'n> {
    loc: Loc,
    what: &'static str,
    allowed: &'static [&'static str],
    entries: Vec<(&'n str, &'n Node, &'n Node)>,
}

impl<'n> Fields<'n> {
    /// Opens `node` as a mapping whose fields are `allowed`. `what` names the
    /// thing for messages ("a unit", "a profile").
    pub fn of(
        node: &'n Node,
        what: &'static str,
        allowed: &'static [&'static str],
        d: &mut Diagnostics,
    ) -> Option<Fields<'n>> {
        let Some(map) = node.as_map() else {
            d.error(
                node.loc,
                format!("{what} must be a mapping; this is {}", node.describe()),
            );
            return None;
        };
        let mut entries = Vec::new();
        for entry in map {
            match entry.key.as_str() {
                Some(key) if allowed.contains(&key.text.as_str()) => {
                    entries.push((key.text.as_str(), &entry.key, &entry.value));
                }
                Some(key) => d.error(
                    entry.key.loc,
                    format!(
                        "`{}` is not a field of {what}; its fields are {}",
                        key.text,
                        allowed.join(", ")
                    ),
                ),
                None => d.error(
                    entry.key.loc,
                    format!(
                        "{what} has named fields; this key is {}",
                        entry.key.describe()
                    ),
                ),
            }
        }
        Some(Fields {
            loc: node.loc,
            what,
            allowed,
            entries,
        })
    }

    /// Opens `node` as [`Fields::of`] does, for a mapping none of whose
    /// fields is required: nothing written there, as in a file that holds
    /// only a comment or a key with nothing after it, is a mapping with no
    /// field in it.
    pub fn all_optional(
        node: &'n Node,
        what: &'static str,
        allowed: &'static [&'static str],
        d: &mut Diagnostics,
    ) -> Option<Fields<'n>> {
        match node.nothing_written() {
            true => Some(Fields {
                loc: node.loc,
                what,
                allowed,
                entries: Vec::new(),
            }),
            false => Fields::of(node, what, allowed, d),
        }
    }

    pub fn loc(&self) -> Loc {
        self.loc
    }

    pub fn what(&self) -> &'static str {
        self.what
    }

    /// The value of a field, if present.
    pub fn get(&self, key: &str) -> Option<&'n Node> {
        self.entry(key).map(|(_, value)| value)
    }

    /// The key node and the value of a field, if present.
    pub fn entry(&self, key: &str) -> Option<(&'n Node, &'n Node)> {
        debug_assert!(self.allowed.contains(&key), "{key} is not an allowed field");
        self.entries
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|(_, key, value)| (*key, *value))
    }

    /// The fields present, with their keys, in the order written.
    pub fn present(&self) -> impl Iterator<Item = (&'n str, &'n Node)> + '_ {
        self.entries.iter().map(|(name, key, _)| (*name, *key))
    }

    /// A required field; its absence is an error at the mapping.
    pub fn require(&self, key: &str, d: &mut Diagnostics) -> Option<&'n Node> {
        let value = self.get(key);
        if value.is_none() {
            d.error(self.loc, format!("`{key}` is required in {}", self.what));
        }
        value
    }
}

/// Text written as a plain string, with no references read from it.
pub fn text<'n>(node: &'n Node, what: &str, d: &mut Diagnostics) -> Option<&'n str> {
    match node.as_str() {
        Some(s) => Some(&s.text),
        None => {
            d.error(
                node.loc,
                format!("{what} must be text; this is {}", node.describe()),
            );
            None
        }
    }
}

/// A value written out in full: text, a whole number or true or false, with
/// nothing in it read as a reference.
pub fn literal(node: &Node, what: &str, d: &mut Diagnostics) -> Option<Literal> {
    match &node.kind {
        Kind::Scalar(Scalar::Str(s)) => Some(Literal::Text(s.text.clone())),
        Kind::Scalar(Scalar::Int(n)) => Some(Literal::Number(*n)),
        Kind::Scalar(Scalar::Bool(b)) => Some(Literal::Bool(*b)),
        _ => {
            d.error(
                node.loc,
                format!(
                    "{what} is text, a whole number or true or false; this is {}",
                    node.describe()
                ),
            );
            None
        }
    }
}

pub fn boolean(node: &Node, what: &str, d: &mut Diagnostics) -> Option<bool> {
    match node.kind {
        Kind::Scalar(Scalar::Bool(b)) => Some(b),
        _ => {
            d.error(
                node.loc,
                format!("{what} must be true or false; this is {}", node.describe()),
            );
            None
        }
    }
}

/// A unit, template, profile or way name.
pub fn name(node: &Node, what: &str, d: &mut Diagnostics) -> Option<NameAt> {
    named_by(node, what, names::name_problem, d)
}

/// A name checked by `rule`.
pub fn named_by(
    node: &Node,
    what: &str,
    rule: fn(&str) -> Option<String>,
    d: &mut Diagnostics,
) -> Option<NameAt> {
    let text = text(node, what, d)?;
    match rule(text) {
        Some(problem) => {
            d.error(node.loc, format!("{what}: {problem}"));
            None
        }
        None => Some(NameAt {
            name: text.to_string(),
            loc: node.loc,
        }),
    }
}

/// A list of names, each checked by `rule`.
pub fn name_list(
    node: &Node,
    what: &str,
    rule: fn(&str) -> Option<String>,
    d: &mut Diagnostics,
) -> Vec<NameAt> {
    let Some(items) = list(node, what, d) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| named_by(item, what, rule, d))
        .collect()
}

pub fn list<'n>(node: &'n Node, what: &str, d: &mut Diagnostics) -> Option<&'n [Node]> {
    match node.as_seq() {
        Some(items) => Some(items),
        None => {
            d.error(
                node.loc,
                format!("{what} must be a list; this is {}", node.describe()),
            );
            None
        }
    }
}

/// A mapping from names (checked by `rule`) to nodes, in the order written.
/// `what` says what each key is. Nothing written there is a mapping of no
/// names, as [`Fields::all_optional`] reads a mapping of no fields.
pub fn keyed<'n>(
    node: &'n Node,
    what: &str,
    rule: fn(&str) -> Option<String>,
    d: &mut Diagnostics,
) -> Vec<(NameAt, &'n Node)> {
    if node.nothing_written() {
        return Vec::new();
    }
    let Some(map) = node.as_map() else {
        d.error(
            node.loc,
            format!(
                "this must be a mapping, each key {what}; this is {}",
                node.describe()
            ),
        );
        return Vec::new();
    };
    map.iter()
        .filter_map(|entry| Some((named_by(&entry.key, what, rule, d)?, &entry.value)))
        .collect()
}
