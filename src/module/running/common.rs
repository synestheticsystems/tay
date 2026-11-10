//! Settings several modules share: command lines, environment variables and
//! mounts. Each module still decides which of these it has.

use crate::diagnostic::Diagnostics;
use crate::syntax::expr::{self, ExprKind};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names;
use crate::syntax::yaml::Node;

use crate::module::shape::{Concern, Place, Slot};
use crate::module::want::Want;

pub const COMMAND_LINE: Place = Place::Readable("a command line");
const ENV: Place =
    Place::Readable("an environment variable in `env` (a secret goes in `secret_env`)");

/// The program and its arguments, as a list. Never a string: tay does not
/// split a command line, and runs nothing through a shell unless asked.
pub fn argv(node: &Node, what: &str, d: &mut Diagnostics) -> Option<Vec<Slot>> {
    let Some(items) = node.as_seq() else {
        d.error(
            node.loc,
            format!(
                "{what} must be a list of the program and its arguments; this is {}",
                node.describe()
            ),
        );
        return None;
    };
    if items.is_empty() {
        d.error(node.loc, format!("{what} must name a program"));
        return None;
    }
    let slots: Vec<Option<Slot>> = items
        .iter()
        .map(|item| Slot::read(item, Want::Scalar, COMMAND_LINE, d))
        .collect();
    slots.into_iter().collect()
}

/// A command for `run`, `check` or `undo`: a list, or `{ shell: "..." }` when a
/// shell is wanted.
pub fn run_form(node: &Node, what: &str, d: &mut Diagnostics) -> Option<Vec<Slot>> {
    if node.as_map().is_none() {
        return argv(node, what, d);
    }
    let shell = Fields::of(node, "a shell command", &["shell"], d)?;
    let text = shell.require("shell", d)?;
    Some(vec![Slot::read(text, Want::Text, COMMAND_LINE, d)?])
}

/// A list of commands, each a list, run in order.
pub fn argv_list(node: &Node, what: &str, d: &mut Diagnostics) -> Option<Vec<Slot>> {
    let items = fields::list(node, what, d)?;
    let commands: Vec<Option<Vec<Slot>>> = items.iter().map(|item| argv(item, what, d)).collect();
    Some(commands.into_iter().collect::<Option<Vec<_>>>()?.concat())
}

/// `env` and `secret_env`. A variable may be set in one of them, not both.
pub fn environment(settings: &Fields<'_>, d: &mut Diagnostics) -> Vec<Slot> {
    let mut slots = Vec::new();
    let plain = settings
        .get("env")
        .map(|node| {
            fields::keyed(
                node,
                "an environment variable",
                names::variable_name_problem,
                d,
            )
        })
        .unwrap_or_default();
    for (_, node) in &plain {
        slots.extend(Slot::read(node, Want::Scalar, ENV, d));
    }
    let secret = settings
        .get("secret_env")
        .map(|node| {
            fields::keyed(
                node,
                "an environment variable",
                names::variable_name_problem,
                d,
            )
        })
        .unwrap_or_default();
    for (name, node) in &secret {
        if let Some(first) = plain.iter().find(|(p, _)| p.name == name.name) {
            d.push(
                crate::diagnostic::Diagnostic::new(
                    name.loc,
                    format!("`{}` is set in both `env` and `secret_env`", name.name),
                )
                .with(first.0.loc, "it is set in `env` here"),
            );
        }
        let Some(expr) = expr::from_node(node, d) else {
            continue;
        };
        if !matches!(expr.kind, ExprKind::Ref(_)) {
            d.error(
                node.loc,
                "`secret_env` takes only a reference to a secret value; a secret written into a definition is not a secret",
            );
            continue;
        }
        slots.push(Slot {
            expr,
            want: Want::Scalar,
            place: Place::SecretEnv,
        });
    }
    slots
}

/// `mounts`: host files or folders made visible in a container.
pub fn mounts(node: &Node, d: &mut Diagnostics) -> (Vec<Slot>, Vec<Concern>) {
    const MOUNT: Place = Place::Readable("a mount");
    let (mut slots, mut concerns) = (Vec::new(), Vec::new());
    let Some(items) = fields::list(node, "`mounts`", d) else {
        return (slots, concerns);
    };
    for item in items {
        let Some(mount) = Fields::of(item, "a mount", &["from", "to", "readonly"], d) else {
            continue;
        };
        if let Some(from) = mount
            .require("from", d)
            .and_then(|n| Slot::read(n, Want::Text, MOUNT, d))
        {
            concerns.push(Concern::MountsHostPath(from.expr.clone()));
            slots.push(from);
        }
        slots.extend(
            mount
                .require("to", d)
                .and_then(|n| Slot::read(n, Want::Text, MOUNT, d)),
        );
        slots.extend(
            mount
                .get("readonly")
                .and_then(|n| Slot::read(n, Want::Bool, MOUNT, d)),
        );
    }
    (slots, concerns)
}
