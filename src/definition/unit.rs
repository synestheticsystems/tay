//! A unit as read: what it is (a group, one module, ways under `run`, or a
//! definition that could not be read), the values it gives, and how it is
//! known to be ready and restarted.

use crate::diagnostic::source::Loc;
use crate::module::{Description, Module, ModuleValue, Output, Slot, Want};
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
#[derive(Debug)]
pub struct Mechanism {
    pub module: &'static dyn Module,
    /// Where `uses` names the module.
    pub uses: Loc,
    pub description: Description,
    pub needs: Vec<NameAt>,
    pub after: Vec<NameAt>,
    pub gives: Vec<Give>,
    pub ready: Option<Ready>,
    pub restart: Option<Restart>,
    /// Files and folders whose change runs the unit again.
    pub watch: Vec<Slot>,
}

impl Mechanism {
    pub fn give(&self, name: &str) -> Option<&Give> {
        self.gives.iter().find(|give| give.name.name == name)
    }

    /// The files the mechanism builds.
    pub fn outputs(&self) -> &[Output] {
        &self.description.outputs
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

impl Endpoint {
    /// What each part must be, as written and once resolved. A `path` must
    /// also start with `/`.
    pub const PORT: Want = Want::PortNumber;
    pub const SCHEME: Want = Want::Text;
    pub const PATH: Want = Want::Text;
    pub const HOST: Want = Want::Text;

    /// What is wrong with a `path` whose text begins with `leading`.
    pub fn path_problem(leading: &str) -> Option<&'static str> {
        (!leading.starts_with('/')).then_some("an endpoint's `path` starts with `/`")
    }
}

#[derive(Clone, Debug)]
pub struct Ready {
    pub check: Check<Slot>,
    pub within: Slot,
}

/// One readiness check, each value as written or once resolved.
#[derive(Clone, Debug)]
pub enum Check<V> {
    /// Any 2xx answer to a GET of this URL, made from the host.
    Http(V),
    /// A connection to this address opens, from the host.
    Tcp(V),
    /// This command exits 0.
    Command(Vec<V>),
    /// A line the unit writes contains this text.
    Log(V),
}

#[derive(Clone, Debug)]
pub struct Restart {
    pub on: Option<Slot>,
    pub attempts: Option<Slot>,
}
