//! Command lines: a program and its arguments, or text a shell runs.

use crate::diagnostic::Diagnostics;
use crate::syntax::fields::{self, Fields};
use crate::syntax::yaml::Node;

use crate::module::settings::{Destination, Slot, each};
use crate::module::want::Want;

pub const COMMAND_LINE: Destination = Destination::Readable("a command line");

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

/// A list of words for a command line, which may be empty, as an
/// entrypoint that clears an image's own is.
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
