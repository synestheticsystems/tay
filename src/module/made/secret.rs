//! `secret`: a value generated once and kept for the environment.

use crate::syntax::fields::{self, Fields};
use crate::syntax::names::NameAt;
use crate::syntax::yaml::Node;

use crate::module::shape::{Given, GivenValue, Place, RestartDefault, Slot, UnitKind, UnitShape};
use crate::module::want::Want;
use crate::module::{Cx, Module};

pub struct Secret;

const SETTING: Place = Place::Readable("a secret's settings");

/// How `bytes` are written as text: `base64` is padded, `base64url` is not.
const ENCODINGS: &[&str] = &["hex", "base64", "base64url"];

const ALGORITHMS: &[&str] = &["ed25519", "rsa-2048"];

impl Module for Secret {
    fn name(&self) -> &'static str {
        "secret"
    }

    fn check(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<UnitShape> {
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
        let mut shape = UnitShape::new(
            UnitKind::Made,
            "when it has been generated",
            RestartDefault::Never,
        );
        // The kind decides which values the unit gives, and those names are
        // checked before anything is resolved, so it is written out. Each is
        // a value's name and whether it is secret.
        let kind_node = settings.require("kind", d)?;
        let kind = fields::text(kind_node, "`kind`", d)?;
        let gives: &[(&str, bool)] = match kind {
            "password" | "bytes" => &[("value", true)],
            "keypair" => &[("public", false), ("private", true)],
            other => {
                d.error(
                    kind_node.loc,
                    format!("`kind` is `password`, `bytes` or `keypair`; this is `{other}`"),
                );
                return None;
            }
        };
        let only_for = |field: &str, kinds: &[&str], d: &mut crate::diagnostic::Diagnostics| {
            let (key, node) = settings.entry(field)?;
            if kinds.contains(&kind) {
                return Some(node);
            }
            let kinds: Vec<String> = kinds.iter().map(|kind| format!("`{kind}`")).collect();
            d.error(
                key.loc,
                format!("`{field}` is for {}, not `{kind}`", kinds.join(" and ")),
            );
            None
        };
        if let Some(length) = only_for("length", &["password", "bytes"], d) {
            shape
                .slots
                .extend(Slot::read(length, Want::Count, SETTING, d));
        }
        if let Some(encoding) = only_for("encoding", &["bytes"], d) {
            shape
                .slots
                .extend(Slot::read(encoding, Want::OneOf(ENCODINGS), SETTING, d));
        }
        match only_for("algorithm", &["keypair"], d) {
            Some(algorithm) => {
                shape
                    .slots
                    .extend(Slot::read(algorithm, Want::OneOf(ALGORITHMS), SETTING, d))
            }
            None if kind == "keypair" && settings.get("algorithm").is_none() => d.error(
                settings.loc(),
                "a `keypair` secret needs `algorithm`: `ed25519` or `rsa-2048`",
            ),
            None => {}
        }
        shape.gives = gives
            .iter()
            .map(|(name, secret)| Given {
                name: NameAt {
                    name: (*name).to_string(),
                    loc: cx.uses,
                },
                value: GivenValue::Generated { secret: *secret },
            })
            .collect();
        (cx.d.len() == errors_before).then_some(shape)
    }
}

#[cfg(test)]
mod tests {
    use crate::module::GivenValue;
    use crate::module::testing::check;

    /// What the unit gives, each with whether it is secret.
    fn gives(with: &str) -> Vec<(String, bool)> {
        check("secret", with)
            .0
            .unwrap()
            .gives
            .into_iter()
            .map(|g| {
                let GivenValue::Generated { secret } = g.value else {
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
