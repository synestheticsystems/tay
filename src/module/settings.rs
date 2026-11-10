//! A module's own settings for one unit: as written, each value a [`Slot`],
//! and once the planner has resolved every slot, each value a [`Value`].
//! The planner turns the one into the other without knowing the module.

use std::fmt;

use crate::diagnostic::Diagnostics;
use crate::syntax::expr::Expr;
use crate::syntax::yaml::Node;
use crate::value::Value;

use super::want::Want;

/// A module's own settings for one unit as written: every value a [`Slot`]
/// that the planner resolves.
pub trait WrittenSettings: fmt::Debug {
    /// Every value, in the order they are written.
    fn slots(&self) -> Vec<&Slot>;

    /// The settings with each value resolved by `resolve`, which reports
    /// why a value cannot be; `None` then. Every value is resolved, so that
    /// every error is found.
    fn resolve(&self, resolve: &mut dyn FnMut(&Slot) -> Option<Value>)
    -> Option<Box<dyn Settings>>;
}

/// A module's own settings for one unit with every value resolved: what the
/// module brings the unit up from, held in the plan. An engine asks these,
/// whatever their module, to start, stop and check the unit.
pub trait Settings: fmt::Debug {}

/// One value a unit's settings use: as written, with what it must be once
/// resolved and where it ends up.
#[derive(Clone, Debug)]
pub struct Slot {
    pub expr: Expr,
    pub want: Want,
    pub destination: Destination,
}

impl Slot {
    /// Reads a setting's value. Its written form is checked now; a reference
    /// in it is checked once it is resolved.
    pub fn read(
        node: &Node,
        want: Want,
        destination: Destination,
        d: &mut Diagnostics,
    ) -> Option<Slot> {
        Some(Slot {
            expr: want.read(node, d)?,
            want,
            destination,
        })
    }
}

/// Where a value ends up, which decides whether a secret may go there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Destination {
    /// `secret_env`: only values with a secret in them.
    SecretEnv,
    /// A `file` unit's content, which is written for its owner alone when
    /// it holds a secret and no `mode` is written.
    FileContent,
    /// Somewhere other programs can read, named for messages.
    Readable(&'static str),
}

/// Each of `values` passed through `f`, every one of them even when one
/// fails, so that each error is reported; `None` if any failed.
pub fn each<'a, V, W>(values: &'a [V], f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<Vec<W>> {
    let all: Vec<Option<W>> = values.iter().map(f).collect();
    all.into_iter().collect()
}

/// [`each`] for values that have names, such as environment variables.
pub fn each_named<'a, N: Clone, V, W>(
    values: &'a [(N, V)],
    f: &mut dyn FnMut(&'a V) -> Option<W>,
) -> Option<Vec<(N, W)>> {
    let all: Vec<Option<(N, W)>> = values
        .iter()
        .map(|(name, value)| Some((name.clone(), f(value)?)))
        .collect();
    all.into_iter().collect()
}

/// A value that may be absent passed through `f`: `Some(None)` when it is
/// absent, `None` when `f` fails.
pub fn maybe<'a, V, W>(
    value: &'a Option<V>,
    f: &mut dyn FnMut(&'a V) -> Option<W>,
) -> Option<Option<W>> {
    match value {
        Some(value) => f(value).map(Some),
        None => Some(None),
    }
}
