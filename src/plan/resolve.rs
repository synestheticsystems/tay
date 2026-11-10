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
mod settings;
mod unit_values;
pub mod value;

pub use environment::Environment;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use crate::definition::{Body, Mechanism, Stack};
use crate::diagnostic::source::Loc;
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::module::{Runs, UnitKind};
use crate::syntax::expr::{Expr, ExprKind, Part, Piece, Reference, Target, Type, View};
use crate::syntax::names::NameAt;

use super::choices::{self, Layered};
use value::{Bit, Hole, Resolved, Shape};

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
            UnitKind::Runs(
                Runs::OwnContainer { .. } | Runs::InsideUnit(_) | Runs::OneOffContainer,
            ) => Asker::Container,
            UnitKind::Made => Asker::Nowhere,
        }
    }
}

/// Why a value cannot be resolved for this asker, though its definition is sound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewProblem {
    /// The host asks for a container's address that is not published.
    NotPublished {
        owner: String,
        port: String,
        endpoint: Loc,
    },
    /// The asker runs nowhere and the reference, written at `at`, names no
    /// view.
    NeedsView {
        reference: String,
        at: Loc,
        endpoint: Loc,
    },
    /// The host asks for a container's address through a published port
    /// that nothing in the plan opens.
    NotOpened { owner: String, port: String },
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
    pub environment: Environment,
    pub d: Diagnostics,
    units: HashMap<&'s str, usize>,
    chosen: HashMap<usize, Option<Chosen<'s>>>,
    /// Results by value, the part of an endpoint asked for, and asker.
    memo: HashMap<(Key, Option<Part>, Asker), Res>,
    resolving: Vec<(Key, Loc)>,
    circles: HashSet<BTreeSet<Key>>,
    /// Each setting with no value and no default that the plan uses, and where.
    unset: BTreeMap<String, Vec<Loc>>,
    /// The folder of each source's file, by source, which a path beginning
    /// `./` is relative to.
    folders: Vec<Option<PathBuf>>,
    /// Once every unit that comes up has used its values: the published
    /// ports something in the plan opens, by container unit and container
    /// port. Until then a use from the host opens the port it reaches.
    pub opened: Option<HashSet<(String, i64)>>,
}

impl<'s> Resolver<'s> {
    pub fn new(
        stack: &'s Stack,
        layered: &'s Layered,
        environment: Environment,
        folders: Vec<Option<PathBuf>>,
    ) -> Resolver<'s> {
        Resolver {
            stack,
            layered,
            environment,
            folders,
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
            opened: None,
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
        self.mechanism(unit).map(|mechanism| &mechanism.shape.kind)
    }

    pub fn resolve(&mut self, expr: &Expr, scope: Scope) -> Res {
        match &expr.kind {
            ExprKind::Number(n) => Ok(Resolved::known(Shape::Number(*n))),
            ExprKind::Bool(b) => Ok(Resolved::known(Shape::Bool(*b))),
            ExprKind::Ref(reference) => self.reference(reference, scope),
            ExprKind::Text(pieces) => self.text(pieces, scope),
        }
    }

    /// Resolves a document's pieces (a template file) as text.
    pub fn text(&mut self, pieces: &[Piece], scope: Scope) -> Res {
        let mut text = Resolved::known(Shape::Text(Vec::new()));
        let mut bits = Vec::new();
        // Every piece is resolved, so that every error in the text is found.
        let mut reported = false;
        let mut views = Vec::new();
        for (index, piece) in pieces.iter().enumerate() {
            match piece {
                Piece::Lit(literal) => bits.push(Bit::Text(literal.clone())),
                Piece::Ref(reference) => match self.reference(reference, scope) {
                    Ok(value) => {
                        text.absorb(&value);
                        text.begins_with_setting |= index == 0 && value.begins_with_setting;
                        bits.extend(value.bits());
                    }
                    Err(Fail::Reported) => reported = true,
                    Err(Fail::View(problems)) => views.extend(problems),
                },
            }
        }
        if !views.is_empty() {
            return Err(Fail::View(views));
        }
        if reported {
            return Err(Fail::Reported);
        }
        text.shape = Shape::Text(bits);
        Ok(text)
    }

    fn reference(&mut self, reference: &Reference, scope: Scope) -> Res {
        match &reference.target {
            Target::Setting(name) => self.setting(name, reference.loc),
            Target::Environment(field) => self.environment_field(*field, reference),
            Target::HostListen => Ok(Resolved::known(Shape::Opaque {
                ty: Some(Type::Text),
                hole: Hole::HostListen,
            })),
            Target::Arg(name) => {
                // An action's arguments were checked against its declaration
                // while reading, in the unit the reference is written in.
                let ty = scope
                    .unit
                    .and_then(|unit| self.stack.units[unit].action.as_ref())
                    .and_then(|action| action.args.iter().find(|arg| arg.name.name == *name))
                    .and_then(|arg| arg.ty);
                Ok(Resolved::known(Shape::Opaque {
                    ty,
                    hole: Hole::Arg(name.clone()),
                }))
            }
            Target::Param(_) => self.fail(
                reference.loc,
                format!(
                    "`{}` is a template's parameter, and this is not in a template",
                    reference.written()
                ),
            ),
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
            return self.fail(
                reference.loc,
                format!("`{}`: there is no unit `{name}`", reference.written()),
            );
        };
        // A way that cannot be chosen has been reported.
        self.chosen(unit).ok_or(Fail::Reported)?;
        match self.kind(unit) {
            Some(kind @ UnitKind::Runs(_)) => Ok(Asker::of(kind)),
            _ => self.fail(
                reference.loc,
                format!(
                    "`{}` asks for the view of unit `{name}`, which runs nowhere; the unit named after `@` must be one that runs",
                    reference.written()
                ),
            ),
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

    fn fail<T>(&mut self, loc: Loc, message: String) -> Result<T, Fail> {
        self.d.push(Diagnostic::new(loc, message));
        Err(Fail::Reported)
    }
}
