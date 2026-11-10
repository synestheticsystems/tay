//! Settings several modules share: command lines, environment variables and
//! mounts. Each module still decides which of these it has.

use crate::diagnostic::Diagnostics;
use crate::expr::{self, Expr, ExprKind};
use crate::fields::{self, Fields};
use crate::names;
use crate::yaml::Node;

use super::shape::{Concern, Form, Place, Slot, Want};

pub const COMMAND_LINE: Place = Place::Readable("a command line");
const ENV: Place =
    Place::Readable("an environment variable in `env` (a secret goes in `secret_env`)");

pub fn slot(expr: Expr, want: Want, place: Place) -> Slot {
    Slot { expr, want, place }
}

/// A value whose written form is checked now; a reference is checked once it
/// is resolved.
pub fn value(node: &Node, want: Want, place: Place, d: &mut Diagnostics) -> Option<Slot> {
    let expr = expr::from_node(node, d)?;
    if let Some(problem) = Form::of_written(&expr).and_then(|form| want.problem(&form)) {
        d.error(node.loc, format!("this {problem}"));
        return None;
    }
    Some(slot(expr, want, place))
}

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
        .map(|item| value(item, Want::Scalar, COMMAND_LINE, d))
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
    Some(vec![value(text, Want::Text, COMMAND_LINE, d)?])
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
        slots.extend(value(node, Want::Scalar, ENV, d));
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
        slots.push(slot(expr, Want::Scalar, Place::SecretEnv));
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
            .and_then(|n| value(n, Want::Text, MOUNT, d))
        {
            concerns.push(Concern::MountsHostPath(from.expr.clone()));
            slots.push(from);
        }
        slots.extend(
            mount
                .require("to", d)
                .and_then(|n| value(n, Want::Text, MOUNT, d)),
        );
        slots.extend(
            mount
                .get("readonly")
                .and_then(|n| value(n, Want::Bool, MOUNT, d)),
        );
    }
    (slots, concerns)
}
