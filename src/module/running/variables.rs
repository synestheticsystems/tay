//! Environment variables: `env`, and `secret_env` for values that are
//! secret.

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::syntax::expr;
use crate::syntax::fields::{self, Fields};
use crate::syntax::names;

use crate::module::settings::{Destination, Slot, each_named};
use crate::module::want::Want;

const ENV: Destination =
    Destination::Readable("an environment variable in `env` (a secret goes in `secret_env`)");

/// `env` and `secret_env`: the environment variables a unit runs with, by
/// name.
#[derive(Debug)]
pub struct Variables<V> {
    pub env: Vec<(String, V)>,
    pub secret_env: Vec<(String, V)>,
}

impl<V> Variables<V> {
    pub fn map<'a, W>(&'a self, f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<Variables<W>> {
        let env = each_named(&self.env, f);
        let secret_env = each_named(&self.secret_env, f);
        Some(Variables {
            env: env?,
            secret_env: secret_env?,
        })
    }
}

/// `env` and `secret_env`. A variable may be set in one of them, not both.
/// A value in `secret_env` must have a secret in it, so one written out in
/// full is an error here; whether a reference brings a secret is known once
/// it is resolved.
pub fn variables(settings: &Fields<'_>, d: &mut Diagnostics) -> Variables<Slot> {
    let mut variables = Variables {
        env: Vec::new(),
        secret_env: Vec::new(),
    };
    let named = |key: &str, d: &mut Diagnostics| {
        settings
            .get(key)
            .map(|node| {
                fields::keyed(
                    node,
                    "an environment variable",
                    names::variable_name_problem,
                    d,
                )
            })
            .unwrap_or_default()
    };
    let plain = named("env", d);
    for (name, node) in &plain {
        if let Some(slot) = Slot::read(node, Want::Scalar, ENV, d) {
            variables.env.push((name.name.clone(), slot));
        }
    }
    for (name, node) in named("secret_env", d) {
        if let Some(first) = plain.iter().find(|(p, _)| p.name == name.name) {
            d.push(
                Diagnostic::new(
                    name.loc,
                    format!("`{}` is set in both `env` and `secret_env`", name.name),
                )
                .with(first.0.loc, "it is set in `env` here"),
            );
        }
        let Some(expr) = expr::from_node(node, d) else {
            continue;
        };
        if expr.references().is_empty() {
            d.error(
                node.loc,
                "a value in `secret_env` must have a secret in it, and this one is written out: a secret written into a definition is not a secret",
            );
            continue;
        }
        let slot = Slot {
            expr,
            want: Want::Scalar,
            destination: Destination::SecretEnv,
        };
        variables.secret_env.push((name.name, slot));
    }
    variables
}
