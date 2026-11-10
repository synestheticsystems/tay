//! Resolving values on the parsed structure: settings, the environment, and
//! what units give, each in the view of whoever uses it.
//!
//! An address depends on who asks, so a value is resolved for an asker: a
//! unit on the host, a unit in a container, or a unit that is neither. A value
//! built from an address (a URL in a unit's `gives`, a setting) carries the
//! address to whoever finally uses it, so switching where one consumer runs
//! never hands it an address meant for another.

mod endpoint;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::definition::{GiveValue, SettingValue, Stack};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::expr::{EnvField, Expr, ExprKind, Part, Piece, Reference, Target, Type, UnitRef, View};
use crate::module::{GivenValue, Runs};
use crate::source::Loc;

use super::choices::{self, Layered};
use super::value::{Bit, Hole, Resolved, Shape};

/// Who uses a value, which decides the view of an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Asker {
    Host,
    Container,
    /// A unit tay handles itself (a port, a file): it runs nowhere, so an
    /// address used there must say which view it wants.
    Unplaced,
}

impl Asker {
    pub fn of(runs: &Runs) -> Asker {
        match runs {
            Runs::OnHost => Asker::Host,
            Runs::OwnContainer { .. } | Runs::InsideUnit(_) | Runs::OneOffContainer => {
                Asker::Container
            }
            Runs::ByTay => Asker::Unplaced,
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

/// What a value is, for finding circles and remembering results.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Key {
    Give(usize, String),
    Setting(String),
}

/// The facts about this environment that references can use.
#[derive(Debug)]
pub struct Environment {
    pub name: String,
    pub label: String,
    pub root: String,
    pub state: String,
    pub profile: Option<String>,
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
    pub unset: BTreeMap<String, Vec<Loc>>,
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

    /// The variant `unit` runs as, and where it was chosen. An error is
    /// reported the first time the choice cannot be made.
    pub fn variant(&mut self, unit: usize) -> Option<(usize, Option<Loc>)> {
        if let Some(chosen) = self.chosen.get(&unit) {
            return *chosen;
        }
        let definition = &self.stack.units[unit];
        let chosen = if definition.variants.is_empty() {
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

    /// Where `unit` runs under its chosen alternative; `None` for a group.
    pub fn runs(&mut self, unit: usize) -> Option<&'s Runs> {
        let stack = self.stack;
        let (variant, _) = self.variant(unit)?;
        stack.units[unit].variants[variant]
            .module
            .as_ref()
            .map(|module| &module.shape.runs)
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
            Target::Setting(name) => self.setting(name, reference.loc, scope.asker),
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
                let stack = self.stack;
                let (variant, _) = self.variant(owner).ok_or(Fail::Reported)?;
                let unit_name = &stack.units[owner].name.name;
                if !stack.units[owner].variants[variant]
                    .outputs
                    .iter()
                    .any(|output| output.name == *name)
                {
                    return self.fail(
                        reference.loc,
                        format!(
                            "`{}`: unit `{unit_name}` builds no output `{name}`",
                            reference.written()
                        ),
                    );
                }
                let mut value = Resolved::known(Shape::Opaque {
                    ty: Some(Type::Text),
                    hole: Hole::Output {
                        unit: unit_name.clone(),
                        name: name.clone(),
                    },
                });
                value
                    .producers
                    .insert(unit_name.clone(), format!("{unit_name}.outputs.{name}"));
                Ok(value)
            }
        }
    }

    fn owner(
        &mut self,
        unit: &UnitRef,
        reference: &Reference,
        scope: Scope,
    ) -> Result<usize, Fail> {
        match unit {
            UnitRef::This => match scope.unit {
                Some(unit) => Ok(unit),
                None => self.fail(
                    reference.loc,
                    format!(
                        "`{}`: `self` is used only inside a unit",
                        reference.written()
                    ),
                ),
            },
            UnitRef::Named(name) => match self.unit_index(name) {
                Some(unit) => Ok(unit),
                None => self.fail(
                    reference.loc,
                    format!("`{}`: there is no unit `{name}`", reference.written()),
                ),
            },
        }
    }

    fn give(
        &mut self,
        owner: usize,
        name: &str,
        part: Option<Part>,
        asker: Asker,
        reference: &Reference,
    ) -> Res {
        let stack = self.stack;
        let (variant, _) = self.variant(owner).ok_or(Fail::Reported)?;
        let unit = &stack.units[owner];
        let Some(give) = unit.variants[variant].give(name) else {
            return self.fail(
                reference.loc,
                format!(
                    "`{}`: unit `{}` gives no `{name}`",
                    reference.written(),
                    unit.name.name
                ),
            );
        };
        if part.is_some() && !matches!(give.value, GiveValue::Endpoint(_)) {
            return self.fail(
                reference.loc,
                format!(
                    "`{}`: `{name}` is not an endpoint, so it has no `.url`, `.host` or `.port`",
                    reference.written()
                ),
            );
        }
        let key = Key::Give(owner, name.to_string());
        let scope = Scope {
            unit: Some(owner),
            asker,
        };
        match &give.value {
            GiveValue::Value(expr) => self.keyed(key, None, reference.loc, asker, |this| {
                this.resolve(expr, scope)
            }),
            GiveValue::Module(GivenValue::Expr(expr)) => {
                self.keyed(key, None, reference.loc, asker, |this| {
                    this.resolve(expr, scope)
                })
            }
            GiveValue::Endpoint(endpoint) => self.keyed(key, part, reference.loc, asker, |this| {
                this.endpoint(owner, endpoint, part, asker, reference)
            }),
            GiveValue::Module(GivenValue::Port) => Ok(Resolved::known(Shape::Opaque {
                ty: Some(Type::Number),
                hole: Hole::Port(unit.name.name.clone()),
            })),
            GiveValue::Module(GivenValue::Secret) => {
                let mut value = Resolved::known(Shape::Opaque {
                    ty: Some(Type::Text),
                    hole: Hole::Secret,
                });
                value.secrets.insert(format!("{}.{name}", unit.name.name));
                Ok(value)
            }
            GiveValue::Module(GivenValue::Produced) => {
                let mut value = Resolved::known(Shape::Opaque {
                    ty: None,
                    hole: Hole::Produced {
                        unit: unit.name.name.clone(),
                        name: name.to_string(),
                    },
                });
                value
                    .producers
                    .insert(unit.name.name.clone(), format!("{}.{name}", unit.name.name));
                Ok(value)
            }
            GiveValue::Module(GivenValue::Output) => {
                unreachable!("outputs are not among the values a unit gives")
            }
        }
    }

    fn setting(&mut self, name: &str, at: Loc, asker: Asker) -> Res {
        let Some(decl) = self.stack.setting(name) else {
            return self.fail(at, format!("no setting `{name}` is declared in tay.yaml"));
        };
        let value = match (self.layered.settings.get(name), &decl.default) {
            (Some(value), _) => value.clone(),
            (None, Some(default)) => SettingValue::Written(default.clone()),
            (None, None) => {
                self.unset.entry(name.to_string()).or_default().push(at);
                return Err(Fail::Reported);
            }
        };
        let scope = Scope { unit: None, asker };
        let resolved = self.keyed(Key::Setting(name.to_string()), None, at, asker, |this| {
            this.resolve(value.expr(), scope)
        })?;
        let ty = match decl.ty {
            Some(ty) => Some(ty),
            None => match &decl.default {
                Some(default) => self.resolve(default, scope)?.ty(),
                None => None,
            },
        };
        let mut resolved = match (&value, ty) {
            (SettingValue::Argument(expr), Some(ty)) => {
                argument(resolved, ty, name, expr.loc, &mut self.d)?
            }
            _ => resolved,
        };
        if let (Some(ty), Some(got)) = (ty, resolved.ty())
            && ty != got
        {
            return self.fail(
                value.expr().loc,
                format!(
                    "setting `{name}` is {}; this value is {}",
                    ty.describe(),
                    got.describe()
                ),
            );
        }
        if decl.secret {
            resolved.shape = Shape::Opaque {
                ty: resolved.ty(),
                hole: Hole::Secret,
            };
            resolved.secrets.insert(format!("settings.{name}"));
        }
        Ok(resolved)
    }

    fn environment_field(&mut self, field: EnvField, reference: &Reference) -> Res {
        let environment = &self.environment;
        Ok(match field {
            EnvField::Name => Resolved::text(&environment.name),
            EnvField::Label => {
                if environment.label.is_empty() {
                    return self.fail(
                        reference.loc,
                        format!(
                            "`{}`: the environment's name `{}` leaves nothing a hostname may hold",
                            reference.written(),
                            environment.name
                        ),
                    );
                }
                Resolved::text(&environment.label)
            }
            EnvField::Root => Resolved::text(&environment.root),
            EnvField::State => Resolved::text(&environment.state),
            EnvField::Id => Resolved::known(Shape::Opaque {
                ty: Some(Type::Text),
                hole: Hole::EnvironmentId,
            }),
            EnvField::Profile => match &environment.profile {
                Some(profile) => Resolved::text(profile),
                None => {
                    return self.fail(
                        reference.loc,
                        format!("`{}`: this stack has no profiles", reference.written()),
                    );
                }
            },
        })
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
        match key {
            Key::Give(unit, name) => format!("{}.{name}", self.stack.units[*unit].name.name),
            Key::Setting(name) => format!("settings.{name}"),
        }
    }

    fn fail<T>(&mut self, loc: Loc, message: String) -> Result<T, Fail> {
        self.d.push(Diagnostic::new(loc, message));
        Err(Fail::Reported)
    }
}

/// The text of a `--set`, typed as its setting is.
fn argument(resolved: Resolved, ty: Type, name: &str, loc: Loc, d: &mut Diagnostics) -> Res {
    let Some(text) = resolved
        .exact_text()
        .filter(|_| resolved.producers.is_empty())
    else {
        return Ok(resolved);
    };
    let shape = match ty {
        Type::Text => return Ok(resolved),
        Type::Number => text.parse().ok().map(Shape::Number),
        Type::Bool => match text.as_str() {
            "true" => Some(Shape::Bool(true)),
            "false" => Some(Shape::Bool(false)),
            _ => None,
        },
    };
    match shape {
        Some(shape) => Ok(Resolved { shape, ..resolved }),
        // The message does not quote the value: it may be a secret.
        None => {
            d.error(
                loc,
                format!(
                    "setting `{name}` is {}, and this value is not",
                    ty.describe()
                ),
            );
            Err(Fail::Reported)
        }
    }
}
