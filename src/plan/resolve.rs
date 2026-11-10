//! Resolving values on the parsed structure: settings, the environment, and
//! what units give, each in the view of whoever uses it.
//!
//! An address depends on who asks, so a value is resolved for an asker: a
//! unit on the host, a unit in a container, or a unit that is neither. A value
//! built from an address (a URL in a unit's `gives`, a setting) carries the
//! address to whoever finally uses it, so switching where one consumer runs
//! never hands it an address meant for another.

pub mod endpoint;
mod environment;
pub mod paths;
pub mod resolved;
mod settings;
mod unit_values;

pub use environment::EnvironmentFacts;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::definition::{Body, Mechanism, Stack, reference_errors};
use crate::diagnostic::source::{Loc, SourceId, Sources};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::module::{Runs, UnitKind};
use crate::syntax::expr::{Expr, ExprKind, Part, Piece, Reference, Target, View};
use crate::syntax::names::NameAt;

use super::choices::{self, Layered};
use crate::value::{Bit, Placeholder, Value};
use resolved::Resolved;

/// Who uses a value, which decides the view of an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Asker {
    Host,
    Container,
    /// A unit that is made: it runs nowhere, so an address used there must
    /// say which view it wants.
    Nowhere,
}

impl Asker {
    pub fn of(kind: &UnitKind) -> Asker {
        match kind {
            UnitKind::Runs(Runs::OnHost) => Asker::Host,
            UnitKind::Runs(Runs::OwnContainer | Runs::InsideUnit(_) | Runs::OneOffContainer) => {
                Asker::Container
            }
            UnitKind::Made => Asker::Nowhere,
        }
    }
}

/// Why a value cannot be resolved for this asker, though its definition is
/// sound. `at` is where the address is used: the reference in the text
/// furthest out that brings it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewProblem {
    /// The host asks for a container's address that is not published.
    NotPublished {
        owner: String,
        port: String,
        at: Loc,
        endpoint: Loc,
    },
    /// The asker runs nowhere and the reference names no view.
    NeedsView {
        reference: String,
        at: Loc,
        endpoint: Loc,
    },
}

impl ViewProblem {
    pub fn at(&self) -> Loc {
        match self {
            ViewProblem::NotPublished { at, .. } | ViewProblem::NeedsView { at, .. } => *at,
        }
    }

    /// The problem, with the address used at `used`: a reference further out
    /// that brings it.
    pub fn used_at(mut self, used: Loc) -> ViewProblem {
        match &mut self {
            ViewProblem::NotPublished { at, .. } | ViewProblem::NeedsView { at, .. } => *at = used,
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// An error in the definitions, already reported.
    Reported,
    /// Each address in the value that this asker cannot have; whoever uses
    /// the value reports them, since only it knows who asked.
    View(Vec<ViewProblem>),
}

pub type Res = Result<Resolved, Fail>;

/// What a unit comes up as, under the way chosen for it.
#[derive(Clone, Copy, Debug)]
pub enum Chosen<'s> {
    /// A group, and what it needs.
    Group(&'s [NameAt]),
    /// The mechanism the unit runs by, and for a unit with ways, the way's
    /// name and where it was chosen.
    Mechanism {
        mechanism: &'s Mechanism,
        way: Option<(&'s NameAt, Loc)>,
    },
}

/// Where a value is resolved: the unit `self` means, and who asks.
#[derive(Clone, Copy, Debug)]
pub struct Scope {
    pub unit: Option<usize>,
    pub asker: Asker,
}

/// A value a unit gives, by the unit's index and the value's name: what
/// circles are found among and results are remembered by.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Key {
    unit: usize,
    name: String,
}

pub struct Resolver<'s> {
    pub stack: &'s Stack,
    pub layered: &'s Layered,
    pub environment: EnvironmentFacts,
    pub d: Diagnostics,
    units: HashMap<&'s str, usize>,
    chosen: HashMap<usize, Option<Chosen<'s>>>,
    /// Results by value, the part of an endpoint asked for, and asker.
    memo: HashMap<(Key, Option<Part>, Asker), Res>,
    resolving: Vec<(Key, Loc)>,
    circles: HashSet<BTreeSet<Key>>,
    /// Each setting with no value and no default that the plan uses, and where.
    unset: BTreeMap<String, Vec<Loc>>,
    /// Every file and argument read, which a path beginning `./` is
    /// relative to the folder of.
    pub sources: &'s mut Sources,
    /// Whether a value was `${host.listen}`, which is known only once the
    /// environment is brought up.
    pub host_listen: bool,
}

impl<'s> Resolver<'s> {
    pub fn new(
        stack: &'s Stack,
        layered: &'s Layered,
        environment: EnvironmentFacts,
        sources: &'s mut Sources,
    ) -> Resolver<'s> {
        Resolver {
            stack,
            layered,
            environment,
            sources,
            host_listen: false,
            d: Diagnostics::default(),
            units: stack
                .units
                .iter()
                .enumerate()
                .map(|(i, unit)| (unit.name.name.as_str(), i))
                .collect(),
            chosen: HashMap::new(),
            memo: HashMap::new(),
            resolving: Vec::new(),
            circles: HashSet::new(),
            unset: BTreeMap::new(),
        }
    }

    pub fn unit_index(&self, name: &str) -> Option<usize> {
        self.units.get(name).copied()
    }

    /// What `unit` comes up as, under the way chosen for it; `None` when
    /// its definition could not be read, or no way can be chosen. An error
    /// is reported the first time the choice cannot be made.
    pub fn chosen(&mut self, unit: usize) -> Option<Chosen<'s>> {
        if let Some(chosen) = self.chosen.get(&unit) {
            return *chosen;
        }
        let definition = &self.stack.units[unit];
        let chosen = match &definition.body {
            Body::Unreadable => None,
            Body::Group { needs } => Some(Chosen::Group(needs)),
            Body::Module(mechanism) => Some(Chosen::Mechanism {
                mechanism,
                way: None,
            }),
            Body::Ways(ways) => match choices::choose(definition, ways, self.layered) {
                Ok((way, at)) => Some(Chosen::Mechanism {
                    mechanism: &way.mechanism,
                    way: Some((&way.name, at)),
                }),
                Err(error) => {
                    self.d.push(error);
                    None
                }
            },
        };
        self.chosen.insert(unit, chosen);
        chosen
    }

    /// The mechanism `unit` runs by, under its chosen way; `None` for a
    /// group, or a unit that cannot be planned.
    pub fn mechanism(&mut self, unit: usize) -> Option<&'s Mechanism> {
        match self.chosen(unit)? {
            Chosen::Mechanism { mechanism, .. } => Some(mechanism),
            Chosen::Group(_) => None,
        }
    }

    /// Whether `unit` runs, and where, under its chosen way; `None` for a
    /// group.
    pub fn kind(&mut self, unit: usize) -> Option<&'s UnitKind> {
        self.mechanism(unit)
            .map(|mechanism| &mechanism.description.kind)
    }

    pub fn resolve(&mut self, expr: &Expr, scope: Scope) -> Res {
        match &expr.kind {
            ExprKind::Number(n) => Ok(Resolved::known(Value::Number(*n))),
            ExprKind::Bool(b) => Ok(Resolved::known(Value::Bool(*b))),
            ExprKind::Ref(reference) => self.reference(reference, scope),
            ExprKind::Text(pieces) => self.text(pieces, expr.loc.source, scope),
        }
    }

    /// Resolves text written in `written_in`.
    fn text(&mut self, pieces: &[Piece], written_in: SourceId, scope: Scope) -> Res {
        let mut text = Resolved::known(Value::Text(Vec::new()));
        let mut bits = Vec::new();
        // Every piece is resolved, so that every error in the text is found.
        let mut reported = false;
        let mut views = Vec::new();
        for (index, piece) in pieces.iter().enumerate() {
            match piece {
                Piece::Lit(literal) => {
                    if index == 0 {
                        text.written_in = Some(written_in);
                    }
                    bits.push(Bit::Text(literal.clone()));
                }
                Piece::Ref(reference) => match self.reference(reference, scope) {
                    Ok(value) => {
                        text.absorb(&value);
                        if index == 0 {
                            text.begins_with_setting = value.begins_with_setting;
                            text.written_in = value.written_in;
                        }
                        bits.extend(value.value.bits());
                    }
                    Err(Fail::Reported) => reported = true,
                    Err(Fail::View(problems)) => views.extend(
                        problems
                            .into_iter()
                            .map(|problem| problem.used_at(reference.loc)),
                    ),
                },
            }
        }
        if !views.is_empty() {
            return Err(Fail::View(views));
        }
        if reported {
            return Err(Fail::Reported);
        }
        text.value = Value::from_bits(bits);
        Ok(text)
    }

    fn reference(&mut self, reference: &Reference, scope: Scope) -> Res {
        match &reference.target {
            Target::Setting(name) => self.setting(name, reference),
            Target::Environment(field) => self.environment_field(*field, reference),
            Target::HostListen => {
                self.host_listen = true;
                Ok(Resolved::known(Value::Placeholder(Placeholder::HostListen)))
            }
            Target::Arg(name) => {
                // An action's arguments were checked against its declaration
                // while reading, in the unit the reference is written in.
                let ty = scope
                    .unit
                    .and_then(|unit| self.stack.units[unit].action.as_ref())
                    .and_then(|action| action.args.iter().find(|arg| arg.name.name == *name))
                    .and_then(|arg| arg.ty);
                Ok(Resolved::known(Value::Placeholder(Placeholder::Arg {
                    name: name.clone(),
                    ty,
                })))
            }
            Target::Param(_) => self.fail(reference_errors::param_outside_a_template(reference)),
            Target::Value {
                unit,
                name,
                part,
                view,
            } => {
                let owner = self.owner(unit, reference, scope)?;
                let asker = self.view(view.as_ref(), scope, reference)?;
                self.give(owner, name, *part, asker, reference)
            }
            Target::Output { unit, name, view } => {
                let owner = self.owner(unit, reference, scope)?;
                // An output is a path on the host, with no endpoint in it to
                // settle; the unit a view names must still be one that runs.
                self.view(view.as_ref(), scope, reference)?;
                self.output(owner, name, reference)
            }
        }
    }

    /// Who a reference's view says the endpoints in its value are seen by:
    /// the host, a container, a unit that runs as it runs, or with no view,
    /// whoever uses the value.
    fn view(
        &mut self,
        view: Option<&View>,
        scope: Scope,
        reference: &Reference,
    ) -> Result<Asker, Fail> {
        let name = match view {
            None => return Ok(scope.asker),
            Some(View::Host) => return Ok(Asker::Host),
            Some(View::Net) => return Ok(Asker::Container),
            Some(View::Unit(name)) => name,
        };
        let Some(unit) = self.unit_index(name) else {
            return self.fail(reference_errors::no_unit(reference, name));
        };
        // A way that cannot be chosen has been reported.
        self.chosen(unit).ok_or(Fail::Reported)?;
        match self.kind(unit) {
            Some(kind @ UnitKind::Runs(_)) => Ok(Asker::of(kind)),
            _ => self.fail(reference_errors::view_of_a_unit_that_runs_nowhere(
                reference, name,
            )),
        }
    }

    /// Resolves the value `key` names once for this part and asker, and
    /// remembers the result; a value built from itself is a circle,
    /// reported.
    fn remembered(
        &mut self,
        key: Key,
        part: Option<Part>,
        at: Loc,
        asker: Asker,
        resolve: impl FnOnce(&mut Self) -> Res,
    ) -> Res {
        if let Some(done) = self.memo.get(&(key.clone(), part, asker)) {
            return done.clone();
        }
        if let Some(start) = self.resolving.iter().position(|(k, _)| *k == key) {
            self.circle(start, at);
            return Err(Fail::Reported);
        }
        self.resolving.push((key.clone(), at));
        let result = resolve(self);
        self.resolving.pop();
        self.memo.insert((key, part, asker), result.clone());
        result
    }

    fn circle(&mut self, start: usize, closing: Loc) {
        let members: Vec<(Key, Loc)> = self.resolving[start..].to_vec();
        if !self
            .circles
            .insert(members.iter().map(|(key, _)| key.clone()).collect())
        {
            return;
        }
        let names: Vec<String> = members.iter().map(|(key, _)| self.key_name(key)).collect();
        let mut path = names.join("`, which is built from `");
        path.push_str(&format!("`, which is built from `{}", names[0]));
        let first = members.get(1).map_or(closing, |(_, loc)| *loc);
        let mut error = Diagnostic::new(
            first,
            format!("a circle among values known before start: `{path}`"),
        );
        if members.len() > 1 {
            error = error.with(closing, "the circle closes here");
        }
        self.d.push(error);
    }

    fn key_name(&self, key: &Key) -> String {
        format!("{}.{}", self.stack.units[key.unit].name.name, key.name)
    }

    fn fail<T>(&mut self, error: Diagnostic) -> Result<T, Fail> {
        self.d.push(error);
        Err(Fail::Reported)
    }
}
