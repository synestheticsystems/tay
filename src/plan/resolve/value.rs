//! A value once its references are resolved, with what the planner needs to
//! know about what it is built from.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::value::Value;

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
    /// The published ports it reaches from the host, by container unit and
    /// container port: using it opens them.
    pub published_uses: BTreeSet<(String, i64)>,
    /// Whether it begins with a setting's value, written there directly or
    /// reached through a value that begins with one.
    pub begins_with_setting: bool,
}

impl Resolved {
    pub fn known(value: Value) -> Resolved {
        Resolved {
            value,
            producers: BTreeMap::new(),
            made: BTreeMap::new(),
            published_uses: BTreeSet::new(),
            begins_with_setting: false,
        }
    }

    pub fn text(text: impl Into<String>) -> Resolved {
        Resolved::known(Value::text(text))
    }

    /// Takes in what `other` is built from: its producers, the units that
    /// are made it uses and the published ports it reaches.
    pub fn absorb(&mut self, other: &Resolved) {
        self.published_uses
            .extend(other.published_uses.iter().cloned());
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
