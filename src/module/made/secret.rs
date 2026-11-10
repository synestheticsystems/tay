//! `secret`: a value generated once and kept for the environment.

mod generate;

use crate::diagnostic::Diagnostics;
use crate::disk::remove_if_there;
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::NameAt;
use crate::syntax::yaml::Node;
use crate::value::{Filled, Placeholder, Unfilled, Value};

use crate::module::description::OpenedPort;
use crate::module::description::{
    Description, ModuleGive, ModuleValue, ReadyWithoutCheck, RestartOn, UnitKind,
};
use crate::module::operations::{Keeping, Operations, PartlyRemoved, Removing, UnitPlace};
use crate::module::settings::{Destination, Keeps, MapValues, Operate, Slot, maybe};
use crate::module::want::Want;
use crate::module::{Cx, Module};

pub struct Secret;

const SETTING: Destination = Destination::Readable("a secret's settings");

/// How `bytes` are written as text: `base64` is padded, `base64url` is not.
const ENCODINGS: &[&str] = &["hex", "base64", "base64url"];

const ALGORITHMS: &[&str] = &["ed25519", "rsa-2048"];

/// The names of the values a secret unit gives: `value` for a password or
/// bytes, `public` and `private` for a key pair.
const VALUE: &str = "value";
const PUBLIC: &str = "public";
const PRIVATE: &str = "private";

/// What a secret unit generates, each value a [`Slot`] as written, a
/// [`Value`](crate::value::Value) once resolved, and [`Filled`] once filled.
#[derive(Debug)]
enum SecretSettings<V> {
    /// Letters and digits. `None` length: 32 characters.
    Password { length: Option<V> },
    /// `None` length: 32 bytes. `None` encoding: `hex`.
    Bytes {
        length: Option<V>,
        encoding: Option<V>,
    },
    /// A key pair, given as PEM text.
    Keypair { algorithm: V },
}

impl<V> MapValues for SecretSettings<V> {
    type Value = V;
    type With<W> = SecretSettings<W>;

    fn map_values<'a, W>(
        &'a self,
        f: &mut dyn FnMut(&'a V) -> Option<W>,
    ) -> Option<SecretSettings<W>> {
        Some(match self {
            SecretSettings::Password { length } => SecretSettings::Password {
                length: maybe(length, f)?,
            },
            SecretSettings::Bytes { length, encoding } => {
                let length = maybe(length, f);
                let encoding = maybe(encoding, f);
                SecretSettings::Bytes {
                    length: length?,
                    encoding: encoding?,
                }
            }
            SecretSettings::Keypair { algorithm } => SecretSettings::Keypair {
                algorithm: f(algorithm)?,
            },
        })
    }
}

/// A unit that is made publishes nothing, so no port is opened for it.
impl Operate for SecretSettings<Filled> {
    fn operate(self, _: Vec<OpenedPort<Filled>>) -> Operations {
        Operations::Made(Box::new(self))
    }
}

/// A secret keeps what was generated for it.
impl Keeps for SecretSettings<Value> {
    fn kept(&self, place: &UnitPlace) -> Keeping {
        Keeping {
            what: vec![format!(
                "the secret it keeps, in `{}`",
                generate::path(place).display()
            )],
            runs: false,
        }
    }

    fn wipe(
        &self,
        place: UnitPlace,
        _: &mut dyn FnMut(&Placeholder) -> Result<Filled, Unfilled>,
    ) -> Result<Removing, Unfilled> {
        Ok(Box::pin(async move {
            remove_if_there(&generate::path(&place)).map_err(PartlyRemoved::after(0))
        }))
    }
}

impl Module for Secret {
    fn name(&self) -> &'static str {
        "secret"
    }

    fn read(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<Description> {
        let settings = read(with, cx)?;
        // Each value the kind gives, and whether it is secret.
        let gives: &[(&str, bool)] = match settings {
            SecretSettings::Password { .. } | SecretSettings::Bytes { .. } => &[(VALUE, true)],
            SecretSettings::Keypair { .. } => &[(PUBLIC, false), (PRIVATE, true)],
        };
        let gives = gives
            .iter()
            .map(|(name, secret)| ModuleGive {
                name: NameAt {
                    name: (*name).to_string(),
                    loc: cx.uses,
                },
                value: ModuleValue::Generated { secret: *secret },
            })
            .collect();
        Some(Description {
            gives,
            ..Description::new(
                UnitKind::Made,
                Box::new(settings),
                ReadyWithoutCheck::Generated,
                RestartOn::Never,
            )
        })
    }
}

fn read(with: Option<&Node>, cx: &mut Cx<'_>) -> Option<SecretSettings<Slot>> {
    let Some(with) = with else {
        cx.d.error(cx.uses, "a `secret` unit needs `with` and in it a `kind`");
        return None;
    };
    let errors_before = cx.d.len();
    let d = &mut *cx.d;
    let settings = Fields::of(
        with,
        "the settings of a `secret` unit",
        &["kind", "length", "encoding", "algorithm"],
        d,
    )?;
    // The kind decides which values the unit gives, and those names are
    // checked before anything is resolved, so it is written out.
    let kind_node = settings.require("kind", d)?;
    let kind = fields::text(kind_node, "`kind`", d)?;
    if !matches!(kind, "password" | "bytes" | "keypair") {
        d.error(
            kind_node.loc,
            format!("`kind` is `password`, `bytes` or `keypair`; this is `{kind}`"),
        );
        return None;
    }
    let only_for = |field: &str, kinds: &[&str], want: Want, d: &mut Diagnostics| {
        let (key, node) = settings.entry(field)?;
        if !kinds.contains(&kind) {
            let kinds: Vec<String> = kinds.iter().map(|kind| format!("`{kind}`")).collect();
            d.error(
                key.loc,
                format!("`{field}` is for {}, not `{kind}`", kinds.join(" and ")),
            );
            return None;
        }
        Slot::read(node, want, SETTING, d)
    };
    let length = only_for("length", &["password", "bytes"], Want::Count, d);
    let encoding = only_for("encoding", &["bytes"], Want::OneOf(ENCODINGS), d);
    let algorithm = only_for("algorithm", &["keypair"], Want::OneOf(ALGORITHMS), d);
    if kind == "keypair" && settings.get("algorithm").is_none() {
        d.error(
            settings.loc(),
            "a `keypair` secret needs `algorithm`: `ed25519` or `rsa-2048`",
        );
    }
    if cx.d.len() != errors_before {
        return None;
    }
    Some(match kind {
        "password" => SecretSettings::Password { length },
        "bytes" => SecretSettings::Bytes { length, encoding },
        _ => SecretSettings::Keypair {
            algorithm: algorithm?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::testing::{self, check};

    /// What the unit gives, each with whether it is secret.
    fn gives(with: &str) -> Vec<(String, bool)> {
        check("secret", with)
            .0
            .unwrap()
            .gives
            .into_iter()
            .map(|g| {
                let ModuleValue::Generated { secret } = g.value else {
                    panic!("a secret's values are generated");
                };
                (g.name.name, secret)
            })
            .collect()
    }

    #[test]
    fn the_kind_decides_what_is_given_and_a_public_key_is_not_secret() {
        assert_eq!(gives("kind: password\n"), [("value".to_string(), true)]);
        assert_eq!(
            gives("kind: bytes\nlength: 16\nencoding: base64url\n"),
            [("value".to_string(), true)]
        );
        assert_eq!(
            gives("kind: keypair\nalgorithm: ed25519\n"),
            [("public".to_string(), false), ("private".to_string(), true)]
        );
    }

    #[test]
    fn each_kind_holds_only_its_own_settings() {
        let read = |with: &str| {
            let (settings, errors) = testing::read(with, |node, cx| read(Some(node), cx));
            assert!(errors.is_empty(), "{errors:?}");
            settings.unwrap()
        };
        assert!(matches!(
            read("kind: password\nlength: 12\n"),
            SecretSettings::Password { length: Some(_) }
        ));
        assert!(matches!(
            read("kind: bytes\n"),
            SecretSettings::Bytes {
                length: None,
                encoding: None
            }
        ));
        assert!(matches!(
            read("kind: keypair\nalgorithm: rsa-2048\n"),
            SecretSettings::Keypair { .. }
        ));
    }

    #[test]
    fn settings_that_do_not_fit_the_kind_are_errors() {
        let (_, errors) = check("secret", "kind: keypair\nlength: 3\n");
        assert_eq!(
            errors,
            [
                "1:1: a `keypair` secret needs `algorithm`: `ed25519` or `rsa-2048`",
                "2:1: `length` is for `password` and `bytes`, not `keypair`",
            ]
        );
        let (_, errors) = check("secret", "kind: bytes\nlength: -4\n");
        assert_eq!(
            errors,
            ["2:9: this must be a whole number above 0; this is -4"]
        );
        let (_, errors) = check("secret", "kind: password\nencoding: hex\n");
        assert_eq!(errors, ["2:1: `encoding` is for `bytes`, not `password`"]);
        let (_, errors) = check("secret", "kind: hex\n");
        assert_eq!(
            errors,
            ["1:7: `kind` is `password`, `bytes` or `keypair`; this is `hex`"]
        );
        let (_, errors) = check("secret", "kind: bytes\nencoding: base32\n");
        assert_eq!(
            errors,
            ["2:11: this must be one of `hex`, `base64`, `base64url`; this is `base32`"]
        );
    }
}
