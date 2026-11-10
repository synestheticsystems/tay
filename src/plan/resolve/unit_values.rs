//! The values units give, and the files they build.

use crate::definition::{GiveValue, reference_errors};
use crate::module::{ModuleValue, UnitKind};
use crate::plan::resolve::value::Resolved;
use crate::syntax::expr::{Part, Reference, UnitRef};
use crate::value::{Hole, Value};

use super::{Asker, Chosen, Fail, Key, Res, Resolver, Scope};

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
                None => self.fail(reference_errors::self_outside_a_unit(reference)),
            },
            UnitRef::Named(name) => match self.unit_index(name) {
                Some(unit) => Ok(unit),
                None => self.fail(reference_errors::no_unit(reference, name)),
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
        let unit = &stack.units[owner];
        let gives = match self.chosen(owner).ok_or(Fail::Reported)? {
            Chosen::Mechanism { mechanism, .. } => &mechanism.gives[..],
            Chosen::Group(_) => &[],
        };
        let Some(give) = gives.iter().find(|give| give.name.name == name) else {
            let mut names: Vec<&str> = gives.iter().map(|give| give.name.name.as_str()).collect();
            names.sort_unstable();
            return self.fail(reference_errors::gives_no(
                reference,
                &unit.name.name,
                name,
                &names,
            ));
        };
        // What is wrong with the reference itself is reported at it, before
        // the value is resolved once and remembered for every reference.
        match &give.value {
            GiveValue::Endpoint(endpoint)
                if part == Some(Part::Url) && endpoint.scheme.is_none() =>
            {
                return self.fail(reference_errors::no_scheme(reference));
            }
            GiveValue::Endpoint(_) => {}
            _ if part.is_some() => {
                return self.fail(reference_errors::not_an_endpoint(reference, name));
            }
            _ => {}
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
            GiveValue::Value(expr) => self.remembered(key, None, reference.loc, asker, |this| {
                this.resolve(expr, scope)
            }),
            GiveValue::Module(ModuleValue::Expr(expr)) => {
                self.remembered(key, None, reference.loc, asker, |this| {
                    this.resolve(expr, scope)
                })
            }
            GiveValue::Endpoint(endpoint) => {
                self.remembered(key, part, reference.loc, asker, |this| {
                    this.endpoint(owner, endpoint, part, asker, reference)
                })
            }
            GiveValue::Module(ModuleValue::Port) => Ok(Resolved::known(Value::Hole(Hole::Port {
                unit: unit.name.name.clone(),
            }))),
            GiveValue::Module(ModuleValue::Generated { secret }) => {
                Ok(Resolved::known(Value::Hole(Hole::Generated {
                    unit: unit.name.name.clone(),
                    name: name.to_string(),
                    secret: *secret,
                })))
            }
            GiveValue::Module(ModuleValue::Produced { secret }) => {
                let mut value = Resolved::known(Value::Hole(Hole::Produced {
                    unit: unit.name.name.clone(),
                    name: name.to_string(),
                    secret: *secret,
                }));
                value
                    .producers
                    .insert(unit.name.name.clone(), format!("{}.{name}", unit.name.name));
                Ok(value)
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

    /// `${unit.outputs.name}`: the whole path of a file the unit builds when
    /// it runs. The path is known before start; the file is not there until
    /// the unit has run, so using it is a need for the unit.
    pub(super) fn output(&mut self, owner: usize, name: &str, reference: &Reference) -> Res {
        let stack = self.stack;
        let unit_name = &stack.units[owner].name.name;
        let outputs = match self.chosen(owner).ok_or(Fail::Reported)? {
            Chosen::Mechanism { mechanism, .. } => mechanism.outputs(),
            Chosen::Group(_) => &[],
        };
        let Some(output) = outputs.iter().find(|output| output.name.name == name) else {
            return self.fail(reference_errors::builds_no_output(
                reference, unit_name, name,
            ));
        };
        let key = Key {
            unit: owner,
            name: format!("outputs.{name}"),
        };
        // An output is a path on the host, whoever uses it.
        let scope = Scope {
            unit: Some(owner),
            asker: Asker::Host,
        };
        let path = self.remembered(key, None, reference.loc, Asker::Host, |this| {
            this.resolve(&output.path, scope)
        })?;
        let mut value = self.host_path(path);
        value
            .producers
            .insert(unit_name.clone(), format!("{unit_name}.outputs.{name}"));
        Ok(value)
    }
}
