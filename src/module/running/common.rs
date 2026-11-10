//! Settings several modules share: command lines, environment variables and
//! mounts. Each module still decides which of these it has.

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::syntax::expr;
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::Node;

use crate::module::shape::{Concern, Place, Slot, UnitShape, VolumeMount};
use crate::module::want::{PathUse, Want};

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
/// A value in `secret_env` must have a secret in it, so one written out in
/// full is an error here; whether a reference brings a secret is known once
/// it is resolved.
pub fn environment(settings: &Fields<'_>, d: &mut Diagnostics) -> Vec<Slot> {
    let mut slots = Vec::new();
    let variables = |key: &str, d: &mut Diagnostics| {
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
    let plain = variables("env", d);
    for (_, node) in &plain {
        slots.extend(Slot::read(node, Want::Scalar, ENV, d));
    }
    for (name, node) in variables("secret_env", d) {
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
        slots.push(Slot {
            expr,
            want: Want::Scalar,
            place: Place::SecretEnv,
        });
    }
    slots
}

/// `mounts`: each a host file or folder made visible in a container,
/// `{ from, to, readonly }`, or a volume another unit owns,
/// `{ volume: "unit/name", to, readonly }`.
pub fn mounts(node: &Node, shape: &mut UnitShape, d: &mut Diagnostics) {
    const MOUNT: Place = Place::Readable("a mount");
    let Some(items) = fields::list(node, "`mounts`", d) else {
        return;
    };
    for item in items {
        let Some(mount) = Fields::of(item, "a mount", &["from", "volume", "to", "readonly"], d)
        else {
            continue;
        };
        match (mount.entry("from"), mount.entry("volume")) {
            (Some((_, from)), None) => {
                if let Some(from) = Slot::read(from, Want::Path(PathUse::Mounted), MOUNT, d) {
                    shape
                        .production
                        .push(Concern::MountsHostPath(from.expr.clone()));
                    shape.slots.push(from);
                }
            }
            (None, Some((_, volume))) => shape.volume_mounts.extend(volume_mount(volume, d)),
            (Some(_), Some((key, _))) => d.error(
                key.loc,
                "a mount has one of `from` and `volume`; this one has both",
            ),
            (None, None) => d.error(
                mount.loc(),
                "a mount needs `from`, a host file or folder, or `volume`, another unit's volume",
            ),
        }
        shape.slots.extend(
            mount
                .require("to", d)
                .and_then(|n| Slot::read(n, Want::Text, MOUNT, d)),
        );
        shape.slots.extend(
            mount
                .get("readonly")
                .and_then(|n| Slot::read(n, Want::Bool, MOUNT, d)),
        );
    }
}

/// `"unit/name"`: the unit that owns the volume, and the volume's name.
fn volume_mount(node: &Node, d: &mut Diagnostics) -> Option<VolumeMount> {
    let text = fields::text(node, "`volume`", d)?;
    let Some((unit, volume)) = text.split_once('/') else {
        d.error(
            node.loc,
            format!("`volume` is written `unit/name`, the unit that owns it and its name; this is `{text}`"),
        );
        return None;
    };
    let name = |name: &str, d: &mut Diagnostics| match names::name_problem(name) {
        None => Some(NameAt {
            name: name.to_string(),
            loc: node.loc,
        }),
        Some(problem) => {
            d.error(node.loc, format!("`volume`: {problem}"));
            None
        }
    };
    let (unit, volume) = (name(unit, d), name(volume, d));
    Some(VolumeMount {
        unit: unit?,
        volume: volume?,
    })
}
