//! A value as far as it is known before start: text, a number or true or
//! false, with a placeholder wherever something is filled in later or is not
//! shown. Each placeholder says what fills it, so that whoever starts a unit
//! can fill `"${a.value}:${b.value}"` from what it has allocated, generated,
//! been asked with or seen produced.

mod filled;

use std::fmt;

use crate::syntax::expr::{Literal, Type};

pub use filled::{Filled, Unfilled};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// Text, perhaps with placeholders in it. It is text whatever fills them.
    Text(Vec<Bit>),
    Number(i64),
    Bool(bool),
    /// A whole value that a placeholder stands for. It has the
    /// placeholder's type.
    Placeholder(Placeholder),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bit {
    Text(String),
    Placeholder(Placeholder),
    /// The port of a URL: `:` and the number, or nothing when the number is
    /// the scheme's usual one. Which, is known once both are filled.
    UrlPort {
        scheme: Box<Value>,
        port: Box<Value>,
    },
}

/// The schemes whose usual port a URL leaves out, and those ports. No other
/// scheme has one.
const USUAL_PORTS: [(&str, i64); 2] = [("http", 80), ("https", 443)];

/// The port a URL of `scheme` leaves out, if it has one.
pub fn usual_port(scheme: &str) -> Option<i64> {
    USUAL_PORTS
        .iter()
        .find(|(name, _)| *name == scheme)
        .map(|(_, port)| *port)
}

/// A part of a value that is not known before start, or is not shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placeholder {
    /// The number of a host port the unit holds for this environment, given
    /// when it is allocated, and the name the unit gives it by.
    Port { unit: String, name: String },
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

impl Placeholder {
    /// The type of what fills the placeholder; `None` when it is not known.
    pub fn ty(&self) -> Option<Type> {
        match self {
            Placeholder::Port { .. } => Some(Type::Number),
            Placeholder::Generated { .. }
            | Placeholder::HostListen
            | Placeholder::EnvironmentId
            | Placeholder::EnvironmentNetwork => Some(Type::Text),
            Placeholder::Produced { .. } => None,
            Placeholder::Arg { ty, .. } => *ty,
            Placeholder::SecretSetting { value, .. } => Some(value.0.ty()),
        }
    }

    /// Whether what fills the placeholder is secret.
    pub fn is_secret(&self) -> bool {
        matches!(
            self,
            Placeholder::SecretSetting { .. }
                | Placeholder::Generated { secret: true, .. }
                | Placeholder::Produced { secret: true, .. }
        )
    }

    /// The secret that fills the placeholder, as a reference names it; `None`
    /// when what fills it is not secret.
    pub fn secret(&self) -> Option<String> {
        match self {
            Placeholder::SecretSetting { name, .. } => Some(format!("settings.{name}")),
            Placeholder::Generated { unit, name, .. }
            | Placeholder::Produced { unit, name, .. }
                if self.is_secret() =>
            {
                Some(format!("{unit}.{name}"))
            }
            _ => None,
        }
    }

    /// The placeholder as `tay plan` prints it: a secret as `<secret>`,
    /// whatever fills it.
    pub fn shown(&self) -> String {
        match self {
            Placeholder::SecretSetting { .. }
            | Placeholder::Generated { secret: true, .. }
            | Placeholder::Produced { secret: true, .. } => "<secret>".to_string(),
            Placeholder::Port { unit, .. } => format!("<port: {unit}>"),
            Placeholder::Generated { unit, name, .. } => format!("<generated: {unit}.{name}>"),
            Placeholder::Produced { unit, name, .. } => format!("<produced: {unit}.{name}>"),
            Placeholder::Arg { name, .. } => format!("<arg: {name}>"),
            Placeholder::HostListen => "<host.listen>".to_string(),
            Placeholder::EnvironmentId => "<environment.id>".to_string(),
            Placeholder::EnvironmentNetwork => "<environment.network>".to_string(),
        }
    }
}

/// A value written out exactly: what a value known before start with no
/// secret in it is, and what a value is once it is filled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exact {
    Text(String),
    Number(i64),
    Bool(bool),
}

impl Exact {
    /// The value as text: text as it is, a number in digits, and true or
    /// false as those words.
    pub fn as_text(&self) -> String {
        match self {
            Exact::Text(text) => text.clone(),
            Exact::Number(n) => n.to_string(),
            Exact::Bool(b) => b.to_string(),
        }
    }
}

impl From<Literal> for Value {
    fn from(literal: Literal) -> Value {
        match literal {
            Literal::Text(text) => Value::text(text),
            Literal::Number(n) => Value::Number(n),
            Literal::Bool(b) => Value::Bool(b),
        }
    }
}

impl Value {
    pub fn text(text: impl Into<String>) -> Value {
        Value::Text(vec![Bit::Text(text.into())])
    }

    /// Text built from `bits`, with text that stands side by side joined, so
    /// that one value has one way of being written.
    pub fn from_bits(bits: impl IntoIterator<Item = Bit>) -> Value {
        let mut joined: Vec<Bit> = Vec::new();
        for bit in bits {
            match (joined.last_mut(), bit) {
                (Some(Bit::Text(before)), Bit::Text(text)) => before.push_str(&text),
                (_, bit) => joined.push(bit),
            }
        }
        Value::Text(joined)
    }

    /// The value as bits of text, for building text from it.
    pub fn bits(&self) -> Vec<Bit> {
        match self {
            Value::Text(bits) => bits.clone(),
            Value::Number(n) => vec![Bit::Text(n.to_string())],
            Value::Bool(b) => vec![Bit::Text(b.to_string())],
            Value::Placeholder(placeholder) => vec![Bit::Placeholder(placeholder.clone())],
        }
    }

    /// Every placeholder in the value, those a URL's port is decided by
    /// among them.
    pub fn placeholders(&self) -> Vec<&Placeholder> {
        match self {
            Value::Text(bits) => bits
                .iter()
                .flat_map(|bit| match bit {
                    Bit::Placeholder(placeholder) => vec![placeholder],
                    Bit::Text(_) => Vec::new(),
                    Bit::UrlPort { scheme, port } => {
                        [scheme.placeholders(), port.placeholders()].concat()
                    }
                })
                .collect(),
            Value::Placeholder(placeholder) => vec![placeholder],
            Value::Number(_) | Value::Bool(_) => Vec::new(),
        }
    }

    /// The secrets the value is built from, as references name them, each
    /// once, sorted.
    pub fn secrets(&self) -> Vec<String> {
        let mut secrets: Vec<String> = self
            .placeholders()
            .iter()
            .filter_map(|h| h.secret())
            .collect();
        secrets.sort();
        secrets.dedup();
        secrets
    }

    pub fn is_secret(&self) -> bool {
        self.placeholders()
            .iter()
            .any(|placeholder| placeholder.is_secret())
    }

    /// The text up to the first placeholder.
    pub fn leading_text(&self) -> String {
        match self {
            Value::Text(bits) => bits
                .iter()
                .map_while(|bit| match bit {
                    Bit::Text(text) => Some(text.as_str()),
                    Bit::Placeholder(_) | Bit::UrlPort { .. } => None,
                })
                .collect(),
            _ => String::new(),
        }
    }

    /// The text, when it has no placeholder.
    pub fn exact_text(&self) -> Option<String> {
        match self {
            Value::Text(bits) => bits
                .iter()
                .map(|bit| match bit {
                    Bit::Text(text) => Some(text.as_str()),
                    Bit::Placeholder(_) | Bit::UrlPort { .. } => None,
                })
                .collect(),
            _ => None,
        }
    }

    /// The value exactly, when it has no placeholder.
    pub fn exact(&self) -> Option<Exact> {
        match self {
            Value::Text(_) => self.exact_text().map(Exact::Text),
            Value::Number(n) => Some(Exact::Number(*n)),
            Value::Bool(b) => Some(Exact::Bool(*b)),
            Value::Placeholder(_) => None,
        }
    }

    /// The value as `tay plan` prints it: each placeholder in angle brackets.
    pub fn shown(&self) -> String {
        self.bits()
            .iter()
            .map(|bit| match bit {
                Bit::Text(text) => text.clone(),
                Bit::Placeholder(placeholder) => placeholder.shown(),
                Bit::UrlPort { port, .. } => format!(":{}", port.shown()),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret_setting(name: &str, value: &str) -> Placeholder {
        Placeholder::SecretSetting {
            name: name.to_string(),
            value: Kept(Literal::Text(value.to_string())),
        }
    }

    #[test]
    fn a_value_with_a_placeholder_is_shown_with_it_and_has_no_exact_form() {
        let value = Value::Text(vec![
            Bit::Text("http://127.0.0.1:".into()),
            Bit::Placeholder(Placeholder::Port {
                unit: "web-port".into(),
                name: "number".into(),
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
    fn text_side_by_side_is_joined() {
        let port = Placeholder::Port {
            unit: "web-port".into(),
            name: "number".into(),
        };
        assert_eq!(
            Value::from_bits([
                Bit::Text("http".into()),
                Bit::Text("://".into()),
                Bit::Placeholder(port.clone()),
                Bit::Text("/".into()),
            ]),
            Value::Text(vec![
                Bit::Text("http://".into()),
                Bit::Placeholder(port),
                Bit::Text("/".into())
            ])
        );
    }

    #[test]
    fn each_secret_placeholder_names_its_secret_and_shows_as_secret() {
        let password = Placeholder::Generated {
            unit: "db-password".into(),
            name: "value".into(),
            secret: true,
        };
        let public = Placeholder::Generated {
            unit: "signing".into(),
            name: "public".into(),
            secret: false,
        };
        let value = Value::Text(vec![
            Bit::Placeholder(secret_setting("key", "hunter2")),
            Bit::Text(":".into()),
            Bit::Placeholder(password.clone()),
            Bit::Placeholder(public),
            Bit::Placeholder(password),
        ]);
        assert_eq!(value.secrets(), ["db-password.value", "settings.key"]);
        assert_eq!(
            value.shown(),
            "<secret>:<secret><generated: signing.public><secret>"
        );
    }

    #[test]
    fn a_secret_settings_value_is_kept_and_not_shown_even_by_debug() {
        let placeholder = secret_setting("key", "hunter2");
        let Placeholder::SecretSetting { value, .. } = &placeholder else {
            unreachable!("the placeholder was made a secret setting");
        };
        assert_eq!(value.0, Literal::Text("hunter2".into()));
        assert!(!format!("{:?}", Value::Placeholder(placeholder.clone())).contains("hunter2"));
        assert_eq!(placeholder.ty(), Some(Type::Text));
    }
}
