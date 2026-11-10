//! The values units give, and the files they build.

use crate::definition::GiveValue;
use crate::module::{GivenValue, UnitKind};
use crate::plan::resolve::value::{Hole, Resolved, Shape};
use crate::syntax::expr::{Part, Reference, Type, UnitRef};

use super::{Asker, Fail, Key, Res, Resolver, Scope};

impl Resolver<'_> {
    pub(super) fn owner(
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

    pub(super) fn give(
        &mut self,
        owner: usize,
        name: &str,
        part: Option<Part>,
        asker: Asker,
        reference: &Reference,
    ) -> Res {
        let stack = self.stack;
        let (way, _) = self.way(owner).ok_or(Fail::Reported)?;
        let unit = &stack.units[owner];
        let Some(give) = unit.ways[way].give(name) else {
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
        let key = Key {
            unit: owner,
            name: name.to_string(),
        };
        let scope = Scope {
            unit: Some(owner),
            asker,
        };
        let mut value = match &give.value {
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
            GiveValue::Module(GivenValue::Generated { secret }) => {
                let hole = match secret {
                    true => Hole::Secret,
                    false => Hole::Generated {
                        unit: unit.name.name.clone(),
                        name: name.to_string(),
                    },
                };
                let mut value = Resolved::known(Shape::Opaque {
                    ty: Some(Type::Text),
                    hole,
                });
                if *secret {
                    value.secrets.insert(format!("{}.{name}", unit.name.name));
                }
                Ok(value)
            }
            GiveValue::Module(GivenValue::Produced { secret }) => {
                let hole = match secret {
                    true => Hole::Secret,
                    false => Hole::Produced {
                        unit: unit.name.name.clone(),
                        name: name.to_string(),
                    },
                };
                let mut value = Resolved::known(Shape::Opaque { ty: None, hole });
                let used = format!("{}.{name}", unit.name.name);
                if *secret {
                    value.secrets.insert(used.clone());
                }
                value.producers.insert(unit.name.name.clone(), used);
                Ok(value)
            }
            GiveValue::Module(GivenValue::Output) => {
                unreachable!("outputs are not among the values a unit gives")
            }
        }?;
        if let Some(UnitKind::Made) = self.kind(owner) {
            value
                .made
                .entry(unit.name.name.clone())
                .or_insert_with(|| format!("{}.{name}", unit.name.name));
        }
        Ok(value)
    }

    /// `${unit.outputs.name}`: a file the unit builds when it runs.
    pub(super) fn output(&mut self, owner: usize, name: &str, reference: &Reference) -> Res {
        let stack = self.stack;
        let (way, _) = self.way(owner).ok_or(Fail::Reported)?;
        let unit_name = &stack.units[owner].name.name;
        if !stack.units[owner].ways[way]
            .outputs
            .iter()
            .any(|output| output.name == name)
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
                name: name.to_string(),
            },
        });
        value
            .producers
            .insert(unit_name.clone(), format!("{unit_name}.outputs.{name}"));
        Ok(value)
    }
}
