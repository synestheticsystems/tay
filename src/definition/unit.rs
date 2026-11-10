//! A unit as read: its ways, the values it gives, and how it is known to be
//! ready and restarted.

use crate::diagnostic::source::Loc;
use crate::module::{Module, ModuleValue, Output, UnitShape};
use crate::syntax::expr::{Expr, Type};
use crate::syntax::names::NameAt;

#[derive(Debug)]
pub struct Unit {
    pub name: NameAt,
    pub doc: Option<String>,
    /// `Some` for a unit that runs only when asked for by name.
    pub action: Option<Action>,
    /// The ways the unit can run: one, unnamed, for a unit with `uses` (or a
    /// group, whose way has no module), and one per name under `run`. Empty
    /// when the unit's definition could not be read, so that other units may
    /// still name it.
    pub ways: Vec<Way>,
}

impl Unit {
    /// Whether the unit lists ways under `run`, so that a choice picks one.
    pub fn has_ways(&self) -> bool {
        self.ways.first().is_some_and(|way| way.name.is_some())
    }

    /// The index of the way named `name`.
    pub fn way(&self, name: &str) -> Option<usize> {
        self.ways
            .iter()
            .position(|way| way.name.as_ref().is_some_and(|own| own.name == name))
    }

    pub fn way_names(&self) -> Vec<&str> {
        self.ways
            .iter()
            .filter_map(|way| way.name.as_ref().map(|name| name.name.as_str()))
            .collect()
    }
}

/// What an action may be asked with.
#[derive(Debug)]
pub struct Action {
    pub args: Vec<Arg>,
}

/// An argument an action declares, used as `${args.name}`.
#[derive(Debug)]
pub struct Arg {
    pub name: NameAt,
    /// The type of its default; `None` when it has none, and so takes the
    /// type of whatever it is asked with.
    pub ty: Option<Type>,
}

/// One way a unit runs, with what the unit writes for all its ways joined in.
#[derive(Debug)]
pub struct Way {
    /// The way's name under `run`; `None` for a unit with `uses`, or a group.
    pub name: Option<NameAt>,
    /// `None` for a group.
    pub module: Option<ModuleUse>,
    pub needs: Vec<NameAt>,
    pub after: Vec<NameAt>,
    pub gives: Vec<Give>,
    pub ready: Option<Ready>,
    pub restart: Option<Restart>,
    pub watch: Vec<Expr>,
}

impl Way {
    pub fn give(&self, name: &str) -> Option<&Give> {
        self.gives.iter().find(|give| give.name.name == name)
    }

    /// The files the way builds; a group builds none.
    pub fn outputs(&self) -> &[Output] {
        self.module
            .as_ref()
            .map_or(&[], |module| &module.shape.outputs)
    }
}

pub struct ModuleUse {
    pub module: &'static dyn Module,
    /// Where `uses` names the module.
    pub uses: Loc,
    pub shape: UnitShape,
}

impl std::fmt::Debug for ModuleUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleUse")
            .field("module", &self.module.name())
            .field("uses", &self.uses)
            .field("shape", &self.shape)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct Give {
    pub name: NameAt,
    pub value: GiveValue,
}

#[derive(Clone, Debug)]
pub enum GiveValue {
    Value(Expr),
    Endpoint(Box<Endpoint>),
    /// A value the unit's module gives.
    Module(ModuleValue),
}

/// A value written as a mapping, with `port`: an address whose host part
/// depends on where the unit that uses it runs, unless it writes its `host`.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub loc: Loc,
    pub port: Expr,
    pub scheme: Option<Expr>,
    pub path: Option<Expr>,
    /// The host everyone uses, when it is written: the endpoint then has no views.
    pub host: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct Ready {
    pub check: Check,
    pub within: Expr,
}

#[derive(Clone, Debug)]
pub enum Check {
    Http(Expr),
    Tcp(Expr),
    Command(Vec<Expr>),
    Log(Expr),
}

#[derive(Clone, Debug)]
pub struct Restart {
    pub on: Option<Expr>,
    pub attempts: Option<Expr>,
}
