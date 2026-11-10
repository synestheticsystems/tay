//! A module's own settings for one unit, in three stages: as written, each
//! value a [`Slot`]; once the planner has resolved every slot, each value a
//! [`Value`]; and once the engine has filled every placeholder, each value
//! [`Filled`], from which the unit's operations act. The planner and the
//! engine turn each stage into the next without knowing the module.

use std::fmt;

use crate::diagnostic::Diagnostics;
use crate::syntax::expr::Expr;
use crate::syntax::yaml::Node;
use crate::value::{Filled, Placeholder, Unfilled, Value};

use super::description::OpenedPort;
use super::operations::{Keeping, Operations, Removing, UnitPlace};
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

/// What a unit keeps, as its module answers from the unit's settings as
/// the planner resolved them, so that it is answered whether or not the
/// values they use are known: what wiping the unit removes.
pub trait Keeps {
    /// What the unit at `place` keeps.
    fn kept(&self, place: &UnitPlace) -> Keeping;

    /// Removes what the unit at `place` keeps, once it is stopped; any
    /// value removing it uses is filled by `fill`, and why one cannot be,
    /// when one cannot, stops it before anything is removed.
    fn wipe(
        &self,
        place: UnitPlace,
        fill: &mut dyn FnMut(&Placeholder) -> Result<Filled, Unfilled>,
    ) -> Result<Removing, Unfilled>;
}

/// A module's own settings for one unit with every value resolved: what the
/// plan holds for the unit without knowing its module.
pub trait Settings: Keeps + fmt::Debug + Send + Sync {
    /// The unit's operations, from these settings with every placeholder
    /// filled by `fill`, and the ports the plan opens for it, filled
    /// already; why a value could not be filled, when one cannot.
    fn fill(
        &self,
        opened: Vec<OpenedPort<Filled>>,
        fill: &mut dyn FnMut(&Placeholder) -> Result<Filled, Unfilled>,
    ) -> Result<Operations, Unfilled>;
}

/// A module's own settings for one unit with every value filled, made into
/// the unit's operations.
pub trait Operate {
    /// The unit's operations, with the ports the plan opens for it, which
    /// only a unit in a container of its own has.
    fn operate(self, opened: Vec<OpenedPort<Filled>>) -> Operations;
}

/// A module's settings, of any stage, which can be rebuilt with each value
/// passed through a function: how each module's settings as written become
/// settings the planner resolves, and resolved ones become settings the
/// engine fills.
pub trait MapValues {
    /// The values the settings hold at this stage.
    type Value;
    /// The same settings with values of type `W`.
    type With<W>;

    /// The settings with each value passed through `f`, every one even when
    /// one fails, so that each failure is reported; `None` if any failed.
    fn map_values<'a, W>(
        &'a self,
        f: &mut dyn FnMut(&'a Self::Value) -> Option<W>,
    ) -> Option<Self::With<W>>;
}

impl<S> WrittenSettings for S
where
    S: MapValues<Value = Slot> + fmt::Debug,
    S::With<Value>: Settings + 'static,
{
    fn slots(&self) -> Vec<&Slot> {
        let mut slots = Vec::new();
        self.map_values(&mut |slot| {
            slots.push(slot);
            Some(())
        });
        slots
    }

    fn resolve(
        &self,
        resolve: &mut dyn FnMut(&Slot) -> Option<Value>,
    ) -> Option<Box<dyn Settings>> {
        Some(Box::new(self.map_values(resolve)?))
    }
}

impl<S> Settings for S
where
    S: MapValues<Value = Value> + Keeps + fmt::Debug + Send + Sync,
    S::With<Filled>: Operate,
{
    fn fill(
        &self,
        opened: Vec<OpenedPort<Filled>>,
        fill: &mut dyn FnMut(&Placeholder) -> Result<Filled, Unfilled>,
    ) -> Result<Operations, Unfilled> {
        Ok(filled(self, fill)?.operate(opened))
    }
}

/// `settings` with every placeholder filled by `fill`; why a value could
/// not be filled, when one cannot.
pub fn filled<S: MapValues<Value = Value>>(
    settings: &S,
    fill: &mut dyn FnMut(&Placeholder) -> Result<Filled, Unfilled>,
) -> Result<S::With<Filled>, Unfilled> {
    let mut unfilled = None;
    let filled = settings.map_values(&mut |value| match value.fill(fill) {
        Ok(filled) => Some(filled),
        Err(why) => {
            unfilled.get_or_insert(why);
            None
        }
    });
    match (filled, unfilled) {
        (Some(filled), _) => Ok(filled),
        (None, Some(why)) => Err(why),
        (None, None) => unreachable!("settings are left unfilled only by a value that was not"),
    }
}

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
