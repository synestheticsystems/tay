//! Resolving values on the parsed structure: settings, the environment, and
//! what units give, each in the view of whoever uses it.
//!
//! An address depends on who asks, so a value is resolved for an asker: a
//! unit on the host, a unit in a container, or a unit that is neither. A value
//! built from an address (a URL in a unit's `gives`, a setting) carries the
//! address to whoever finally uses it, so switching where one consumer runs
//! never hands it an address meant for another.

mod endpoint;
mod environment;
mod settings;
mod unit_values;
pub mod value;

pub use environment::Environment;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::definition::Stack;
use crate::diagnostic::source::Loc;
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::module::{Runs, UnitKind};
use crate::syntax::expr::{Expr, ExprKind, Part, Piece, Reference, Target, Type, View};

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
    /// The asker runs nowhere and the reference names no view.
    NeedsView { reference: String, endpoint: Loc },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// An error in the definitions, already reported.
    Reported,
    View(ViewProblem),
}

pub type Res = Result<Resolved, Fail>;

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
    chosen: HashMap<usize, Option<(usize, Option<Loc>)>>,
    /// Results by value, the part of an endpoint asked for, and asker.
    memo: HashMap<(Key, Option<Part>, Asker), Res>,
    resolving: Vec<(Key, Loc)>,
    circles: HashSet<BTreeSet<Key>>,
    /// Each setting with no value and no default that the plan uses, and where.
    unset: BTreeMap<String, Vec<Loc>>,
}

impl<'s> Resolver<'s> {
    pub fn new(stack: &'s Stack, layered: &'s Layered, environment: Environment) -> Resolver<'s> {
        Resolver {
            stack,
            layered,
            environment,
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

    /// The way `unit` runs, and where it was chosen. An error is
    /// reported the first time the choice cannot be made.
    pub fn way(&mut self, unit: usize) -> Option<(usize, Option<Loc>)> {
        if let Some(chosen) = self.chosen.get(&unit) {
            return *chosen;
        }
        let definition = &self.stack.units[unit];
        let chosen = if definition.ways.is_empty() {
            None
        } else {
            match choices::choose(definition, self.layered) {
                Ok(chosen) => Some(chosen),
                Err(error) => {
                    self.d.push(error);
                    None
                }
            }
        };
        self.chosen.insert(unit, chosen);
        chosen
    }

    /// Whether `unit` runs, and where, under its chosen way; `None` for a
    /// group.
    pub fn kind(&mut self, unit: usize) -> Option<&'s UnitKind> {
        let stack = self.stack;
        let (way, _) = self.way(unit)?;
        stack.units[unit].ways[way]
            .module
            .as_ref()
            .map(|module| &module.shape.kind)
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
        let mut failure = None;
        for piece in pieces {
            match piece {
                Piece::Lit(literal) => bits.push(Bit::Text(literal.clone())),
                Piece::Ref(reference) => match self.reference(reference, scope) {
                    Ok(value) => {
                        text.absorb(&value);
                        bits.extend(value.bits());
                    }
                    // Keep going, so every error in the text is reported.
                    Err(fail) => failure = failure.or(Some(fail)),
                },
            }
        }
        if let Some(fail) = failure {
            return Err(fail);
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
                let asker = match view {
                    Some(View::Host) => Asker::Host,
                    Some(View::Net) => Asker::Container,
                    None => scope.asker,
                };
                self.give(owner, name, *part, asker, reference)
            }
            Target::Output { unit, name } => {
                let owner = self.owner(unit, reference, scope)?;
                self.output(owner, name, reference)
            }
        }
    }

    /// Resolves the value `key` names, remembering the result for this part
    /// and asker, and reporting a circle when the value is built from itself.
    fn keyed(
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
