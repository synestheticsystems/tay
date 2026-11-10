//! A value once its references are resolved, as far as it can be before start.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::module::Form;
use crate::syntax::expr::Type;

/// A part of a value that is not known, or not shown, until start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hole {
    /// A port this environment has not allocated yet.
    Port(String),
    Secret,
    /// A value generated once and kept, such as a key pair's public key,
    /// that is not secret but is not made until the environment is first
    /// brought up.
    Generated {
        unit: String,
        name: String,
    },
    /// A value a unit writes out when it runs.
    Produced {
        unit: String,
        name: String,
    },
    /// An action's argument, given when the action is asked for.
    Arg(String),
    HostListen,
    EnvironmentId,
    EnvironmentNetwork,
}

impl Hole {
    pub fn shown(&self) -> String {
        match self {
            Hole::Port(unit) => format!("<port: {unit}>"),
            Hole::Secret => "<secret>".to_string(),
            Hole::Generated { unit, name } => format!("<generated: {unit}.{name}>"),
            Hole::Produced { unit, name } => format!("<produced: {unit}.{name}>"),
            Hole::Arg(name) => format!("<arg: {name}>"),
            Hole::HostListen => "<host.listen>".to_string(),
            Hole::EnvironmentId => "<environment.id>".to_string(),
            Hole::EnvironmentNetwork => "<environment.network>".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bit {
    Text(String),
    Hole(Hole),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    Text(Vec<Bit>),
    Number(i64),
    Bool(bool),
    /// A whole value not known or not shown before start. `ty` is `None` for
    /// a value produced by running, whose type is not known.
    Opaque {
        ty: Option<Type>,
        hole: Hole,
    },
}

/// A resolved value. Nothing secret is ever held in one: a secret is a hole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub shape: Shape,
    /// The secrets it is built from, by the references that name them.
    pub secrets: BTreeSet<String>,
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

/// A value exactly, when all of it is known before start and none of it is secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exact {
    Text(String),
    Number(i64),
    Bool(bool),
}

impl Resolved {
    pub fn known(shape: Shape) -> Resolved {
        Resolved {
            shape,
            secrets: BTreeSet::new(),
            producers: BTreeMap::new(),
            made: BTreeMap::new(),
            published_uses: BTreeSet::new(),
            begins_with_setting: false,
        }
    }

    pub fn text(text: impl Into<String>) -> Resolved {
        Resolved::known(Shape::Text(vec![Bit::Text(text.into())]))
    }

    pub fn is_secret(&self) -> bool {
        !self.secrets.is_empty()
    }

    /// Takes in what `other` is built from: its secrets, its producers, the
    /// units that are made it uses and the published ports it reaches.
    pub fn absorb(&mut self, other: &Resolved) {
        self.secrets.extend(other.secrets.iter().cloned());
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

    /// The value as bits of text, for building text from it.
    pub fn bits(&self) -> Vec<Bit> {
        match &self.shape {
            Shape::Text(bits) => bits.clone(),
            Shape::Number(n) => vec![Bit::Text(n.to_string())],
            Shape::Bool(b) => vec![Bit::Text(b.to_string())],
            Shape::Opaque { hole, .. } => vec![Bit::Hole(hole.clone())],
        }
    }

    /// What a [`crate::module::Want`] is checked against.
    pub fn form(&self) -> Form {
        match &self.shape {
            Shape::Text(_) => Form::Text(self.exact_text()),
            Shape::Number(n) => Form::Number(Some(*n)),
            Shape::Bool(_) => Form::Bool,
            Shape::Opaque { ty, .. } => match ty {
                Some(Type::Text) => Form::Text(None),
                Some(Type::Number) => Form::Number(None),
                Some(Type::Bool) => Form::Bool,
                None => Form::Unknown,
            },
        }
    }

    /// The text, when every bit of it is known.
    pub fn exact_text(&self) -> Option<String> {
        match &self.shape {
            Shape::Text(bits) => bits
                .iter()
                .map(|bit| match bit {
                    Bit::Text(text) => Some(text.as_str()),
                    Bit::Hole(_) => None,
                })
                .collect(),
            _ => None,
        }
    }

    pub fn exact(&self) -> Option<Exact> {
        if self.is_secret() {
            return None;
        }
        match &self.shape {
            Shape::Text(_) => self.exact_text().map(Exact::Text),
            Shape::Number(n) => Some(Exact::Number(*n)),
            Shape::Bool(b) => Some(Exact::Bool(*b)),
            Shape::Opaque { .. } => None,
        }
    }

    /// The value as `tay plan` prints it: holes in angle brackets.
    pub fn shown(&self) -> String {
        self.bits()
            .iter()
            .map(|bit| match bit {
                Bit::Text(text) => text.clone(),
                Bit::Hole(hole) => hole.shown(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_with_a_hole_is_shown_with_it_and_has_no_exact_form() {
        let value = Resolved::known(Shape::Text(vec![
            Bit::Text("http://127.0.0.1:".into()),
            Bit::Hole(Hole::Port("web-port".into())),
        ]));
        assert_eq!(value.shown(), "http://127.0.0.1:<port: web-port>");
        assert_eq!(value.exact(), None);
        assert_eq!(value.form(), Form::Text(None));
    }

    #[test]
    fn a_value_built_from_a_secret_has_no_exact_form() {
        let mut value = Resolved::text("plain");
        value.secrets.insert("settings.key".into());
        assert_eq!(value.exact(), None);
        assert_eq!(
            Resolved::text("plain").exact(),
            Some(Exact::Text("plain".into()))
        );
    }
}
