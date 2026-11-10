//! A value once its references are resolved, with what the planner needs to
//! know about what it is built from.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::diagnostic::source::SourceId;
use crate::plan::model::UsedFile;
use crate::value::Value;

/// An entry of a container's `publish` that a value reaches from the host:
/// using the value opens it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReachedEntry {
    /// The unit whose container publishes the port.
    pub container: String,
    pub port: i64,
    /// Whether its entry opens it with no use, by `always: true`.
    pub always: bool,
}

/// A resolved value, and what it is built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub value: Value,
    /// The units whose running produces part of it, each with the value it
    /// produces. Empty when the value is known before start.
    pub producers: BTreeMap<String, String>,
    /// The units that are made whose values it is built from, each with the
    /// value used.
    pub made: BTreeMap<String, String>,
    /// The files tay knows of that it is built from, by name.
    pub files: BTreeMap<String, UsedFile>,
    /// The `publish` entries it reaches from the host: using it opens them.
    pub reached_entries: BTreeSet<ReachedEntry>,
    /// Whether it begins with a setting's value, written there directly or
    /// reached through a value that begins with one.
    pub begins_with_setting: bool,
    /// Where the text it begins with was written, which a path beginning
    /// `./` is relative to; `None` when it begins with nothing written.
    pub written_in: Option<SourceId>,
}

impl Resolved {
    pub fn known(value: Value) -> Resolved {
        Resolved {
            value,
            producers: BTreeMap::new(),
            made: BTreeMap::new(),
            files: BTreeMap::new(),
            reached_entries: BTreeSet::new(),
            begins_with_setting: false,
            written_in: None,
        }
    }

    pub fn text(text: impl Into<String>) -> Resolved {
        Resolved::known(Value::text(text))
    }

    /// Takes in what `other` is built from: its producers, the units that
    /// are made it uses, the files it uses and the published ports it
    /// reaches.
    pub fn absorb(&mut self, other: &Resolved) {
        self.files.extend(other.files.clone());
        self.reached_entries
            .extend(other.reached_entries.iter().cloned());
        for (unit, value) in &other.producers {
            self.producers
                .entry(unit.clone())
                .or_insert_with(|| value.clone());
        }
        for (unit, value) in &other.made {
            self.made
                .entry(unit.clone())
                .or_insert_with(|| value.clone());
        }
    }
}
