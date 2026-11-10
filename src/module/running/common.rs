//! Settings several modules share: command lines, environment variables and
//! mounts. Each module still decides which of these it has.

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::syntax::expr;
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::Node;

use crate::module::description::{Concern, VolumeMount};
use crate::module::settings::{Destination, Slot, each, each_named, maybe};
use crate::module::want::{PathUse, Want};

pub const COMMAND_LINE: Destination = Destination::Readable("a command line");
const ENV: Destination =
    Destination::Readable("an environment variable in `env` (a secret goes in `secret_env`)");
const MOUNT: Destination = Destination::Readable("a mount");

/// The program and its arguments, as a list. Never a string: tay does not
/// split a command line, and runs nothing through a shell unless asked.
pub fn argv(node: &Node, what: &str, d: &mut Diagnostics) -> Option<Vec<Slot>> {
    let words = words(node, what, d)?;
    if words.is_empty() {
        d.error(node.loc, format!("{what} must name a program"));
        return None;
    }
    Some(words)
}

/// A list of words for a command line, which may be empty.
pub fn words(node: &Node, what: &str, d: &mut Diagnostics) -> Option<Vec<Slot>> {
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
    let slots: Vec<Option<Slot>> = items
        .iter()
        .map(|item| Slot::read(item, Want::Scalar, COMMAND_LINE, d))
        .collect();
    slots.into_iter().collect()
}

/// A command for `run`, `check` or `undo`: a list, or `{ shell: "..." }`.
#[derive(Debug)]
pub enum RunForm<V> {
    /// The program and its arguments.
    Argv(Vec<V>),
    /// Text a shell runs.
    Shell(V),
}

impl<V> RunForm<V> {
    pub fn map<'a, W>(&'a self, f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<RunForm<W>> {
        Some(match self {
            RunForm::Argv(words) => RunForm::Argv(each(words, f)?),
            RunForm::Shell(text) => RunForm::Shell(f(text)?),
        })
    }
}

/// A command for `run`, `check` or `undo`: a list, or `{ shell: "..." }` when a
/// shell is wanted.
pub fn run_form(node: &Node, what: &str, d: &mut Diagnostics) -> Option<RunForm<Slot>> {
    if node.as_map().is_none() {
        return argv(node, what, d).map(RunForm::Argv);
    }
    let shell = Fields::of(node, "a shell command", &["shell"], d)?;
    let text = shell.require("shell", d)?;
    Some(RunForm::Shell(Slot::read(
        text,
        Want::Text,
        COMMAND_LINE,
        d,
    )?))
}

/// A list of commands, each a list, run in order.
pub fn argv_list(node: &Node, what: &str, d: &mut Diagnostics) -> Option<Vec<Vec<Slot>>> {
    let items = fields::list(node, what, d)?;
    let commands: Vec<Option<Vec<Slot>>> = items.iter().map(|item| argv(item, what, d)).collect();
    commands.into_iter().collect()
}

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

/// A host file or folder, or another unit's volume, made visible in a
/// container.
#[derive(Debug)]
pub struct Mount<V> {
    pub source: Source<V>,
    pub to: V,
    /// `None`: read-only, the default.
    pub readonly: Option<V>,
}

/// What a mount makes visible.
#[derive(Debug)]
pub enum Source<V> {
    /// A file or folder on the host.
    Host(V),
    /// A volume another unit owns.
    Volume(VolumeMount),
}

impl<V> Mount<V> {
    pub fn map<'a, W>(&'a self, f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<Mount<W>> {
        let source = match &self.source {
            Source::Host(from) => f(from).map(Source::Host),
            Source::Volume(volume) => Some(Source::Volume(volume.clone())),
        };
        let to = f(&self.to);
        let readonly = maybe(&self.readonly, f);
        Some(Mount {
            source: source?,
            to: to?,
            readonly: readonly?,
        })
    }
}

impl Mount<Slot> {
    /// What a production profile refuses in this mount: a host path.
    pub fn concern(&self) -> Option<Concern> {
        match &self.source {
            Source::Host(from) => Some(Concern::MountsHostPath(from.expr.clone())),
            Source::Volume(_) => None,
        }
    }

    /// The other unit's volume this mount makes visible.
    pub fn volume(&self) -> Option<&VolumeMount> {
        match &self.source {
            Source::Volume(volume) => Some(volume),
            Source::Host(_) => None,
        }
    }
}

/// `mounts`: each a host file or folder made visible in a container,
/// `{ from, to, readonly }`, or a volume another unit owns,
/// `{ volume: "unit/name", to, readonly }`.
pub fn mounts(node: &Node, d: &mut Diagnostics) -> Vec<Mount<Slot>> {
    let Some(items) = fields::list(node, "`mounts`", d) else {
        return Vec::new();
    };
    let mut mounts = Vec::new();
    for item in items {
        let Some(mount) = Fields::of(item, "a mount", &["from", "volume", "to", "readonly"], d)
        else {
            continue;
        };
        let source = match (mount.entry("from"), mount.entry("volume")) {
            (Some((_, from)), None) => {
                Slot::read(from, Want::Path(PathUse::Mounted), MOUNT, d).map(Source::Host)
            }
            (None, Some((_, volume))) => volume_mount(volume, d).map(Source::Volume),
            (Some(_), Some((key, _))) => {
                d.error(
                    key.loc,
                    "a mount has one of `from` and `volume`; this one has both",
                );
                None
            }
            (None, None) => {
                d.error(
                    mount.loc(),
                    "a mount needs `from`, a host file or folder, or `volume`, another unit's volume",
                );
                None
            }
        };
        let to = mount
            .require("to", d)
            .and_then(|n| Slot::read(n, Want::Text, MOUNT, d));
        let readonly = match mount.get("readonly") {
            Some(n) => Slot::read(n, Want::Bool, MOUNT, d).map(Some),
            None => Some(None),
        };
        if let (Some(source), Some(to), Some(readonly)) = (source, to, readonly) {
            mounts.push(Mount {
                source,
                to,
                readonly,
            });
        }
    }
    mounts
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
