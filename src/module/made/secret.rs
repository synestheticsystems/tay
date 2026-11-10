//! `secret`: a value generated once and kept for the environment.

use crate::syntax::fields::{self, Fields};
use crate::syntax::names::NameAt;
use crate::syntax::yaml::Node;

use crate::module::shape::{Given, GivenValue, Place, RestartDefault, Runs, Slot, UnitShape};
use crate::module::want::Want;
use crate::module::{Cx, Module};

pub struct Secret;

const SETTING: Place = Place::Readable("a secret's settings");

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
            &["kind", "length", "algorithm"],
            d,
        )?;
        let mut slots = Vec::new();
        // The kind decides which values the unit gives, and those names are
        // checked before anything is resolved, so it is written out.
        let kind_node = settings.require("kind", d)?;
        let gives: &[&str] = match fields::text(kind_node, "`kind`", d)? {
            "password" | "hex" => &["value"],
            "keypair" => &["public", "private"],
            other => {
                d.error(
                    kind_node.loc,
                    format!("`kind` is `password`, `hex` or `keypair`; this is `{other}`"),
                );
                return None;
            }
        };
        let keypair = gives.len() == 2;
        match (settings.entry("length"), keypair) {
            (Some((key, _)), true) => d.error(
                key.loc,
                "`length` is for `password` and `hex`, not `keypair`",
            ),
            (Some((_, length)), false) => {
                slots.extend(Slot::read(length, Want::Number, SETTING, d))
            }
            (None, _) => {}
        }
        match (settings.entry("algorithm"), keypair) {
            (Some((_, algorithm)), true) => slots.extend(Slot::read(
                algorithm,
                Want::OneOf(&["ed25519", "rsa-2048"]),
                SETTING,
                d,
            )),
            (Some((key, _)), false) => d.error(key.loc, "`algorithm` is for `keypair`"),
            (None, true) => d.error(
                settings.loc(),
                "a `keypair` secret needs `algorithm`: `ed25519` or `rsa-2048`",
            ),
            (None, false) => {}
        }
        (cx.d.len() == errors_before).then(|| UnitShape {
            runs: Runs::ByTay,
            slots,
            gives: gives
                .iter()
                .map(|name| Given {
                    name: NameAt {
                        name: (*name).to_string(),
                        loc: cx.uses,
                    },
                    value: GivenValue::Secret,
                })
                .collect(),
            outputs: Vec::new(),
            production: Vec::new(),
            ready_without_check: "when it has been generated",
            restart_by_default: RestartDefault::NEVER,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::module::testing::check;

    fn gives(with: &str) -> Vec<String> {
        check("secret", with)
            .0
            .unwrap()
            .gives
            .into_iter()
            .map(|g| g.name.name)
            .collect()
    }

    #[test]
    fn the_kind_decides_what_is_given() {
        assert_eq!(gives("kind: password\n"), ["value"]);
        assert_eq!(gives("kind: hex\nlength: 16\n"), ["value"]);
        assert_eq!(
            gives("kind: keypair\nalgorithm: ed25519\n"),
            ["public", "private"]
        );
    }

    #[test]
    fn settings_that_do_not_fit_the_kind_are_errors() {
        let (_, errors) = check("secret", "kind: keypair\nlength: 3\n");
        assert_eq!(
            errors,
            [
                "1:1: a `keypair` secret needs `algorithm`: `ed25519` or `rsa-2048`",
                "2:1: `length` is for `password` and `hex`, not `keypair`",
            ]
        );
        let (_, errors) = check("secret", "kind: token\n");
        assert_eq!(
            errors,
            ["1:7: `kind` is `password`, `hex` or `keypair`; this is `token`"]
        );
        let (_, errors) = check("secret", "kind: keypair\nalgorithm: dsa\n");
        assert_eq!(
            errors,
            ["2:12: this must be one of `ed25519`, `rsa-2048`; this is `dsa`"]
        );
    }
}
