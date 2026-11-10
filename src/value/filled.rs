//! A value once every placeholder in it has been filled: concrete text, a
//! number or true or false, ready for a module to act on. A value with a
//! secret in it stays a secret when it is filled, so that nothing prints it.

use std::fmt;

use super::{Bit, Exact, Kept, Placeholder, Value};
use crate::syntax::expr::Literal;

/// A value with every placeholder in it filled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filled {
    /// A value with no secret in it.
    Plain(Exact),
    /// A value with a secret in it, kept from every output.
    Secret(Secret),
}

/// A value with a secret in it. Nothing shows it: it has no `Display`, its
/// `Debug` writes `<secret>`, and its text is had only through `reveal`,
/// by whatever puts it where a secret may go.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Exact);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<secret>")
    }
}

/// Why a placeholder cannot be filled.
#[derive(Debug, thiserror::Error)]
pub enum Unfilled {
    /// The unit that gives the value has not given it.
    #[error("`{unit}.{name}` has no value yet")]
    Value { unit: String, name: String },
    /// An action's argument, which only asking for the action gives.
    #[error(
        "`${{args.{name}}}` is given when the action is asked for with its arguments, which tay cannot do yet"
    )]
    Argument { name: String },
    /// A fact of the environment that this tay cannot work out here.
    #[error("{reference} cannot be worked out: {why}")]
    Fact {
        reference: &'static str,
        why: &'static str,
    },
}

impl Filled {
    pub fn text(text: impl Into<String>) -> Filled {
        Filled::Plain(Exact::Text(text.into()))
    }

    pub fn number(number: i64) -> Filled {
        Filled::Plain(Exact::Number(number))
    }

    pub fn secret(text: impl Into<String>) -> Filled {
        Filled::Secret(Secret(Exact::Text(text.into())))
    }

    /// A secret setting's value, kept for whoever runs a unit, filled in.
    pub fn kept(kept: &Kept) -> Filled {
        Filled::Secret(Secret(match &kept.0 {
            Literal::Text(text) => Exact::Text(text.clone()),
            Literal::Number(n) => Exact::Number(*n),
            Literal::Bool(b) => Exact::Bool(*b),
        }))
    }

    /// The value, when it has no secret in it.
    pub fn plain(&self) -> Option<&Exact> {
        match self {
            Filled::Plain(exact) => Some(exact),
            Filled::Secret(_) => None,
        }
    }

    pub fn is_secret(&self) -> bool {
        matches!(self, Filled::Secret(_))
    }

    /// The value as text, a secret's included: for whatever puts it where
    /// a secret may go, such as a file written for its owner alone.
    pub fn reveal(&self) -> String {
        match self {
            Filled::Plain(exact) | Filled::Secret(Secret(exact)) => exact.as_text(),
        }
    }

    fn into_secret(self) -> Filled {
        match self {
            Filled::Plain(exact) => Filled::Secret(Secret(exact)),
            secret @ Filled::Secret(_) => secret,
        }
    }
}

impl Value {
    /// The value with each placeholder in it filled by `fill`. Text stays
    /// text whatever fills it, and holds a secret when anything in it is
    /// one; a value that is one placeholder has the type of what fills it.
    pub fn fill(
        &self,
        fill: &mut dyn FnMut(&Placeholder) -> Result<Filled, Unfilled>,
    ) -> Result<Filled, Unfilled> {
        let mut filled = |placeholder: &Placeholder| {
            let value = fill(placeholder)?;
            Ok(match placeholder.is_secret() {
                true => value.into_secret(),
                false => value,
            })
        };
        match self {
            Value::Number(n) => Ok(Filled::number(*n)),
            Value::Bool(b) => Ok(Filled::Plain(Exact::Bool(*b))),
            Value::Placeholder(placeholder) => filled(placeholder),
            Value::Text(bits) => {
                let mut text = String::new();
                let mut secret = false;
                for bit in bits {
                    let value = match bit {
                        Bit::Text(written) => {
                            text.push_str(written);
                            continue;
                        }
                        Bit::Placeholder(placeholder) => filled(placeholder)?,
                        Bit::UrlPort { scheme, port } => {
                            url_port(&scheme.fill(&mut filled)?, port.fill(&mut filled)?)
                        }
                    };
                    secret |= value.is_secret();
                    text.push_str(&value.reveal());
                }
                Ok(match secret {
                    true => Filled::secret(text),
                    false => Filled::text(text),
                })
            }
        }
    }
}

/// The port of a URL of `scheme`, filled: `:` and the number, or nothing
/// when it is the scheme's usual one. A secret is never compared, and the
/// text holds a secret when the port is one.
fn url_port(scheme: &Filled, port: Filled) -> Filled {
    let usual = match (scheme.plain(), port.plain()) {
        (Some(Exact::Text(scheme)), Some(Exact::Number(number))) => {
            super::usual_port(scheme) == Some(*number)
        }
        _ => false,
    };
    match (usual, port) {
        (true, _) => Filled::text(""),
        (false, Filled::Plain(port)) => Filled::text(format!(":{}", port.as_text())),
        (false, Filled::Secret(Secret(port))) => Filled::secret(format!(":{}", port.as_text())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port() -> Placeholder {
        Placeholder::Port {
            unit: "web-port".into(),
            name: "number".into(),
        }
    }

    fn password() -> Placeholder {
        Placeholder::Generated {
            unit: "db-password".into(),
            name: "value".into(),
            secret: true,
        }
    }

    /// Fills a port with 20117 and a password with `hunter2`, as their
    /// units give them.
    fn given(placeholder: &Placeholder) -> Result<Filled, Unfilled> {
        match placeholder {
            Placeholder::Port { .. } => Ok(Filled::number(20117)),
            _ => Ok(Filled::secret("hunter2")),
        }
    }

    #[test]
    fn a_value_that_is_one_placeholder_has_the_type_of_what_fills_it() {
        let filled = Value::Placeholder(port()).fill(&mut given).unwrap();
        assert_eq!(filled, Filled::number(20117));
        assert_eq!(filled.plain(), Some(&Exact::Number(20117)));
    }

    #[test]
    fn text_is_filled_in_place_and_holds_a_secret_when_any_of_it_is_one() {
        let url = Value::Text(vec![
            Bit::Text("http://127.0.0.1:".into()),
            Bit::Placeholder(port()),
            Bit::Text("/".into()),
        ]);
        assert_eq!(
            url.fill(&mut given).unwrap(),
            Filled::text("http://127.0.0.1:20117/")
        );
        let with_password = Value::Text(vec![
            Bit::Text("db://app:".into()),
            Bit::Placeholder(password()),
            Bit::Text("@db".into()),
        ]);
        let filled = with_password.fill(&mut given).unwrap();
        assert!(filled.is_secret() && filled.plain().is_none());
        assert_eq!(filled.reveal(), "db://app:hunter2@db");
    }

    #[test]
    fn a_secret_stays_one_whatever_fills_it_and_nothing_shows_it() {
        // The plan says the value is secret; it stays so even if what fills
        // it came back plain.
        let mut plain_password = |_: &Placeholder| Ok(Filled::text("hunter2"));
        let filled = Value::Placeholder(password())
            .fill(&mut plain_password)
            .unwrap();
        assert!(filled.is_secret());
        assert_eq!(format!("{filled:?}"), "Secret(<secret>)");
        let setting = Placeholder::SecretSetting {
            name: "licence_key".into(),
            value: Kept(Literal::Text("hunter2".into())),
        };
        let mut from_setting = |_: &Placeholder| Ok(Filled::text("hunter2"));
        let filled = Value::Placeholder(setting).fill(&mut from_setting).unwrap();
        assert!(!format!("{filled:?}").contains("hunter2"));
    }

    #[test]
    fn a_urls_port_is_left_out_when_it_is_filled_with_the_schemes_usual_one() {
        let url = |scheme: &str| {
            Value::Text(vec![
                Bit::Text(format!("{scheme}://127.0.0.1")),
                Bit::UrlPort {
                    scheme: Box::new(Value::text(scheme)),
                    port: Box::new(Value::Placeholder(port())),
                },
                Bit::Text("/health".into()),
            ])
        };
        let filled_with = |number: i64| {
            move |_: &Placeholder| -> Result<Filled, Unfilled> { Ok(Filled::number(number)) }
        };
        let cases = [
            ("https", 443, "https://127.0.0.1/health"),
            ("https", 8443, "https://127.0.0.1:8443/health"),
            ("http", 80, "http://127.0.0.1/health"),
            ("http", 443, "http://127.0.0.1:443/health"),
        ];
        for (scheme, number, wanted) in cases {
            let filled = url(scheme).fill(&mut filled_with(number)).unwrap();
            assert_eq!(filled, Filled::text(wanted));
        }
        assert_eq!(
            url("https").shown(),
            "https://127.0.0.1:<port: web-port>/health"
        );
    }

    #[test]
    fn a_placeholder_that_cannot_be_filled_says_why() {
        let arg = Placeholder::Arg {
            name: "user".into(),
            ty: None,
        };
        let mut not_asked = |placeholder: &Placeholder| match placeholder {
            Placeholder::Arg { name, .. } => Err(Unfilled::Argument { name: name.clone() }),
            _ => Ok(Filled::text("")),
        };
        let error = Value::Text(vec![Bit::Text("--user=".into()), Bit::Placeholder(arg)])
            .fill(&mut not_asked)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "`${args.user}` is given when the action is asked for with its arguments, which tay cannot do yet"
        );
    }
}
