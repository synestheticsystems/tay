//! `secret`: a value generated once and kept for the environment.

use crate::diagnostic::Diagnostics;
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::NameAt;
use crate::syntax::yaml::Node;
use crate::value::Value;

use crate::module::description::{
    Description, ModuleGive, ModuleValue, ReadyWithoutCheck, RestartOn, UnitKind,
};
use crate::module::settings::{Destination, Settings, Slot, WrittenSettings, maybe};
use crate::module::want::Want;
use crate::module::{Cx, Module};

pub struct Secret;

const SETTING: Destination = Destination::Readable("a secret's settings");

/// How `bytes` are written as text: `base64` is padded, `base64url` is not.
const ENCODINGS: &[&str] = &["hex", "base64", "base64url"];

const ALGORITHMS: &[&str] = &["ed25519", "rsa-2048"];

/// What a secret unit generates, each value a [`Slot`] as written or a
/// [`Value`] once resolved.
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

impl<V> SecretSettings<V> {
    fn map<'a, W>(&'a self, f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<SecretSettings<W>> {
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

impl WrittenSettings for SecretSettings<Slot> {
    fn slots(&self) -> Vec<&Slot> {
        let mut slots = Vec::new();
        self.map(&mut |slot| {
            slots.push(slot);
            Some(())
        });
        slots
    }

    fn resolve(
        &self,
        resolve: &mut dyn FnMut(&Slot) -> Option<Value>,
    ) -> Option<Box<dyn Settings>> {
        Some(Box::new(self.map(resolve)?))
    }
}

impl Settings for SecretSettings<Value> {}

impl Module for Secret {
    fn name(&self) -> &'static str {
        "secret"
    }

    fn read(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<Description> {
        let settings = read(with, cx)?;
        // Each value the kind gives, and whether it is secret.
        let gives: &[(&str, bool)] = match settings {
            SecretSettings::Password { .. } | SecretSettings::Bytes { .. } => &[("value", true)],
            SecretSettings::Keypair { .. } => &[("public", false), ("private", true)],
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
