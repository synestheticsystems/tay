//! A value as far as it is known before start: text, a number or true or
//! false, with a hole wherever something is filled in later or is not shown.
//! Each hole says what fills it, so that whoever starts a unit can fill
//! `"${a.value}:${b.value}"` from what it has allocated, generated, been
//! asked with or seen produced.

use std::fmt;

use crate::syntax::expr::{Literal, Type};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// Text, perhaps with holes in it. It is text whatever fills them.
    Text(Vec<Bit>),
    Number(i64),
    Bool(bool),
    /// A whole value that a hole stands for. It has the hole's type.
    Hole(Hole),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bit {
    Text(String),
    Hole(Hole),
}

/// A part of a value that is not known before start, or is not shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hole {
    /// The number of a host port the unit holds for this environment, given
    /// when it is allocated.
    Port { unit: String },
    /// A value a unit generates once and keeps, the first time the
    /// environment is brought up. A key pair's public half is not secret.
    Generated {
        unit: String,
        name: String,
        secret: bool,
    },
    /// A value a unit writes out when it runs. Its type is not known.
    Produced {
        unit: String,
        name: String,
        secret: bool,
    },
    /// An action's argument, given when the action is asked for; of the
    /// type of its default, or of no known type when it has none.
    Arg { name: String, ty: Option<Type> },
    /// `${host.listen}`.
    HostListen,
    /// `${environment.id}`.
    EnvironmentId,
    /// `${environment.network}`.
    EnvironmentNetwork,
    /// A setting declared secret. Its value is known before start and kept
    /// for whoever runs the unit; it is never shown.
    SecretSetting { name: String, value: Kept },
}

/// A secret's value, kept for whoever runs a unit. Nothing prints it, not
/// even `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Kept(pub Literal);

impl fmt::Debug for Kept {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Kept(..)")
    }
}

impl Hole {
    /// The type of what fills the hole; `None` when it is not known.
    pub fn ty(&self) -> Option<Type> {
        match self {
            Hole::Port { .. } => Some(Type::Number),
            Hole::Generated { .. }
            | Hole::HostListen
            | Hole::EnvironmentId
            | Hole::EnvironmentNetwork => Some(Type::Text),
            Hole::Produced { .. } => None,
            Hole::Arg { ty, .. } => *ty,
            Hole::SecretSetting { value, .. } => Some(value.0.ty()),
        }
    }

    /// The secret that fills the hole, as a reference names it; `None`
    /// when what fills it is not secret.
    pub fn secret(&self) -> Option<String> {
        match self {
            Hole::SecretSetting { name, .. } => Some(format!("settings.{name}")),
            Hole::Generated {
                unit,
                name,
                secret: true,
            }
            | Hole::Produced {
                unit,
                name,
                secret: true,
            } => Some(format!("{unit}.{name}")),
            _ => None,
        }
    }

    /// The hole as `tay plan` prints it.
    pub fn shown(&self) -> String {
        if self.secret().is_some() {
            return "<secret>".to_string();
        }
        match self {
            Hole::Port { unit } => format!("<port: {unit}>"),
            Hole::Generated { unit, name, .. } => format!("<generated: {unit}.{name}>"),
            Hole::Produced { unit, name, .. } => format!("<produced: {unit}.{name}>"),
            Hole::Arg { name, .. } => format!("<arg: {name}>"),
            Hole::HostListen => "<host.listen>".to_string(),
            Hole::EnvironmentId => "<environment.id>".to_string(),
            Hole::EnvironmentNetwork => "<environment.network>".to_string(),
            Hole::SecretSetting { .. } => "<secret>".to_string(),
        }
    }
}

/// A value exactly, when all of it is known before start and none of it is
/// secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exact {
    Text(String),
    Number(i64),
    Bool(bool),
}

impl Value {
    pub fn text(text: impl Into<String>) -> Value {
        Value::Text(vec![Bit::Text(text.into())])
    }

    /// The value as bits of text, for building text from it.
    pub fn bits(&self) -> Vec<Bit> {
        match self {
            Value::Text(bits) => bits.clone(),
            Value::Number(n) => vec![Bit::Text(n.to_string())],
            Value::Bool(b) => vec![Bit::Text(b.to_string())],
            Value::Hole(hole) => vec![Bit::Hole(hole.clone())],
        }
    }

    pub fn holes(&self) -> Vec<&Hole> {
        match self {
            Value::Text(bits) => bits
                .iter()
                .filter_map(|bit| match bit {
                    Bit::Hole(hole) => Some(hole),
                    Bit::Text(_) => None,
                })
                .collect(),
            Value::Hole(hole) => vec![hole],
            Value::Number(_) | Value::Bool(_) => Vec::new(),
        }
    }

    /// The secrets the value is built from, as references name them, each
    /// once and in order.
    pub fn secrets(&self) -> Vec<String> {
        let mut secrets: Vec<String> = self.holes().iter().filter_map(|h| h.secret()).collect();
        secrets.sort();
        secrets.dedup();
        secrets
    }

    pub fn is_secret(&self) -> bool {
        self.holes().iter().any(|hole| hole.secret().is_some())
    }

    /// The text up to the first hole.
    pub fn leading_text(&self) -> String {
        match self {
            Value::Text(bits) => bits
                .iter()
                .map_while(|bit| match bit {
                    Bit::Text(text) => Some(text.as_str()),
                    Bit::Hole(_) => None,
                })
                .collect(),
            _ => String::new(),
        }
    }

    /// The text, when it has no hole.
    pub fn exact_text(&self) -> Option<String> {
        match self {
            Value::Text(bits) => bits
                .iter()
                .map(|bit| match bit {
                    Bit::Text(text) => Some(text.as_str()),
                    Bit::Hole(_) => None,
                })
                .collect(),
            _ => None,
        }
    }

    /// The value exactly, when it has no hole.
    pub fn exact(&self) -> Option<Exact> {
        match self {
            Value::Text(_) => self.exact_text().map(Exact::Text),
            Value::Number(n) => Some(Exact::Number(*n)),
            Value::Bool(b) => Some(Exact::Bool(*b)),
            Value::Hole(_) => None,
        }
    }

    /// The value as `tay plan` prints it: each hole in angle brackets.
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

    fn secret_setting(name: &str, value: &str) -> Hole {
        Hole::SecretSetting {
            name: name.to_string(),
            value: Kept(Literal::Text(value.to_string())),
        }
    }

    #[test]
    fn a_value_with_a_hole_is_shown_with_it_and_has_no_exact_form() {
        let value = Value::Text(vec![
            Bit::Text("http://127.0.0.1:".into()),
            Bit::Hole(Hole::Port {
                unit: "web-port".into(),
            }),
        ]);
        assert_eq!(value.shown(), "http://127.0.0.1:<port: web-port>");
        assert_eq!(value.exact(), None);
        assert_eq!(
            Value::text("plain").exact(),
            Some(Exact::Text("plain".into()))
        );
    }

    #[test]
    fn each_secret_hole_names_its_secret_and_shows_as_secret() {
        let password = Hole::Generated {
            unit: "db-password".into(),
            name: "value".into(),
            secret: true,
        };
        let public = Hole::Generated {
            unit: "signing".into(),
            name: "public".into(),
            secret: false,
        };
        let value = Value::Text(vec![
            Bit::Hole(secret_setting("key", "hunter2")),
            Bit::Text(":".into()),
            Bit::Hole(password.clone()),
            Bit::Hole(public),
            Bit::Hole(password),
        ]);
        assert_eq!(value.secrets(), ["db-password.value", "settings.key"]);
        assert_eq!(
            value.shown(),
            "<secret>:<secret><generated: signing.public><secret>"
        );
    }

    #[test]
    fn a_secret_settings_value_is_kept_and_not_shown_even_by_debug() {
        let hole = secret_setting("key", "hunter2");
        let Hole::SecretSetting { value, .. } = &hole else {
            unreachable!("the hole was made a secret setting");
        };
        assert_eq!(value.0, Literal::Text("hunter2".into()));
        assert!(!format!("{:?}", Value::Hole(hole.clone())).contains("hunter2"));
        assert_eq!(hole.ty(), Some(Type::Text));
    }
}
