//! A unit as read: what it is (a group, one module, ways under `run`, or a
//! definition that could not be read), the values it gives, and how it is
//! known to be ready and restarted.

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
    pub body: Body,
}

/// What a unit is, as its definition writes it.
#[derive(Debug)]
pub enum Body {
    /// A unit with none of `uses`, `run` and `from`: asking for it asks for
    /// what it needs.
    Group { needs: Vec<NameAt> },
    /// A unit with `uses`: one module.
    Module(Box<Mechanism>),
    /// A unit with `run`: the ways it can run, at least one, of which a
    /// choice picks one.
    Ways(Vec<Way>),
    /// A unit whose definition has errors, which have been reported. It is
    /// kept so that other units may still name it.
    Unreadable,
}

impl Unit {
    /// Every mechanism the unit may run by: its module, or each of its ways.
    pub fn mechanisms(&self) -> Vec<&Mechanism> {
        match &self.body {
            Body::Module(mechanism) => vec![mechanism],
            Body::Ways(ways) => ways.iter().map(|way| &way.mechanism).collect(),
            Body::Group { .. } | Body::Unreadable => Vec::new(),
        }
    }

    /// The way named `name`, for a unit with ways.
    pub fn way(&self, name: &str) -> Option<&Way> {
        match &self.body {
            Body::Ways(ways) => ways.iter().find(|way| way.name.name == name),
            _ => None,
        }
    }

    pub fn way_names(&self) -> Vec<&str> {
        match &self.body {
            Body::Ways(ways) => ways.iter().map(|way| way.name.name.as_str()).collect(),
            _ => Vec::new(),
        }
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

/// One way under `run`: its name, and the mechanism it is, with what the
/// unit writes for all its ways joined in.
#[derive(Debug)]
pub struct Way {
    pub name: NameAt,
    pub mechanism: Mechanism,
}

/// A module and the fields that go with it: what a unit with `uses` runs
/// by, and what each of a unit's ways is.
pub struct Mechanism {
    pub module: &'static dyn Module,
    /// Where `uses` names the module.
    pub uses: Loc,
    pub shape: UnitShape,
    pub needs: Vec<NameAt>,
    pub after: Vec<NameAt>,
    pub gives: Vec<Give>,
    pub ready: Option<Ready>,
    pub restart: Option<Restart>,
    pub watch: Vec<Expr>,
}

impl Mechanism {
    pub fn give(&self, name: &str) -> Option<&Give> {
        self.gives.iter().find(|give| give.name.name == name)
    }

    /// The files the mechanism builds.
    pub fn outputs(&self) -> &[Output] {
        &self.shape.outputs
    }
}

impl std::fmt::Debug for Mechanism {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mechanism")
            .field("module", &self.module.name())
            .field("uses", &self.uses)
            .field("shape", &self.shape)
            .field("needs", &self.needs)
            .field("after", &self.after)
            .field("gives", &self.gives)
            .field("ready", &self.ready)
            .field("restart", &self.restart)
            .field("watch", &self.watch)
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
