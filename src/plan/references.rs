//! Every name and reference in the definitions points at something that
//! exists: checked across all units and all their alternatives, whether or
//! not they come up, so a mistake shows before the unit is ever used.

use crate::definition::{Check, Choices, GiveValue, SettingValue, Stack, Unit, Variant};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::expr::{Expr, Reference, Target, UnitRef};
use crate::module::{GivenValue, Runs};
use crate::names::NameAt;

pub fn check(stack: &Stack, places: &[&Choices], d: &mut Diagnostics) {
    for unit in &stack.units {
        for variant in &unit.variants {
            for name in variant.needs.iter().chain(&variant.after) {
                unit_named(stack, name, d);
            }
            if let Some(module) = &variant.module
                && let Runs::InsideUnit(inner) = &module.shape.runs
            {
                unit_named(stack, inner, d);
            }
            for expr in exprs(variant) {
                references(stack, expr, Some(unit), d);
            }
        }
    }
    for setting in &stack.settings {
        if let Some(default) = &setting.default {
            references(stack, default, None, d);
        }
    }
    for place in places {
        choices(stack, place, d);
    }
}

/// Every value written in a variant.
fn exprs(variant: &Variant) -> Vec<&Expr> {
    let mut all: Vec<&Expr> = Vec::new();
    if let Some(module) = &variant.module {
        all.extend(module.shape.slots.iter().map(|slot| &slot.expr));
    }
    for give in &variant.gives {
        match &give.value {
            GiveValue::Value(expr) | GiveValue::Module(GivenValue::Expr(expr)) => all.push(expr),
            GiveValue::Endpoint(endpoint) => {
                all.push(&endpoint.port);
                all.extend(endpoint.scheme.iter().chain(&endpoint.path));
            }
            GiveValue::Module(_) => {}
        }
    }
    if let Some(ready) = &variant.ready {
        match &ready.check {
            Check::Http(expr) | Check::Tcp(expr) | Check::Log(expr) => all.push(expr),
            Check::Command(words) => all.extend(words),
        }
        all.push(&ready.within);
    }
    if let Some(restart) = &variant.restart {
        all.extend(restart.on.iter().chain(&restart.attempts));
    }
    all.extend(&variant.watch);
    all
}

fn unit_named<'s>(stack: &'s Stack, name: &NameAt, d: &mut Diagnostics) -> Option<&'s Unit> {
    let unit = stack.unit(&name.name);
    if unit.is_none() {
        d.error(name.loc, format!("there is no unit `{}`", name.name));
    }
    unit
}

fn references(stack: &Stack, expr: &Expr, this: Option<&Unit>, d: &mut Diagnostics) {
    for reference in expr.references() {
        reference_target(stack, reference, this, d);
    }
}

fn reference_target(
    stack: &Stack,
    reference: &Reference,
    this: Option<&Unit>,
    d: &mut Diagnostics,
) {
    let written = reference.written();
    let owner = |unit: &UnitRef, d: &mut Diagnostics| match unit {
        UnitRef::This => {
            if this.is_none() {
                d.error(
                    reference.loc,
                    format!("`{written}`: `self` is used only inside a unit"),
                );
            }
            this
        }
        UnitRef::Named(name) => {
            let found = stack.unit(name);
            if found.is_none() {
                d.error(
                    reference.loc,
                    format!("`{written}`: there is no unit `{name}`"),
                );
            }
            found
        }
    };
    match &reference.target {
        Target::Value {
            unit, name, part, ..
        } => {
            let Some(owner) = owner(unit, d) else { return };
            if owner.variants.is_empty() {
                return;
            }
            let gives: Vec<_> = owner
                .variants
                .iter()
                .filter_map(|variant| variant.give(name))
                .collect();
            if gives.is_empty() {
                let mut names: Vec<&str> = owner
                    .variants
                    .iter()
                    .flat_map(|v| v.gives.iter().map(|g| g.name.name.as_str()))
                    .collect();
                names.sort_unstable();
                names.dedup();
                let list = if names.is_empty() {
                    "nothing".to_string()
                } else {
                    names
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                d.error(
                    reference.loc,
                    format!(
                        "`{written}`: unit `{}` gives no `{name}`; it gives {list}",
                        owner.name.name
                    ),
                );
            } else if part.is_some()
                && !gives
                    .iter()
                    .any(|g| matches!(g.value, GiveValue::Endpoint(_)))
            {
                d.error(
                    reference.loc,
                    format!("`{written}`: `{name}` is not an endpoint, so it has no `.url`, `.host` or `.port`"),
                );
            }
        }
        Target::Output { unit, name } => {
            let Some(owner) = owner(unit, d) else { return };
            if !owner.variants.is_empty()
                && !owner
                    .variants
                    .iter()
                    .any(|v| v.outputs.iter().any(|o| o.name == *name))
            {
                d.error(
                    reference.loc,
                    format!(
                        "`{written}`: unit `{}` builds no output `{name}`",
                        owner.name.name
                    ),
                );
            }
        }
        Target::Setting(name) => {
            if stack.setting(name).is_none() {
                d.error(
                    reference.loc,
                    format!("`{written}`: no setting `{name}` is declared in tay.yaml"),
                );
            }
        }
        Target::Param(_) => d.error(
            reference.loc,
            format!("`{written}` is a template's parameter, and this is not in a template"),
        ),
        Target::Environment(_) | Target::HostListen => {}
    }
}

fn choices(stack: &Stack, place: &Choices, d: &mut Diagnostics) {
    for name in place
        .up
        .iter()
        .flatten()
        .chain(&place.up_add)
        .chain(&place.up_remove)
    {
        unit_named(stack, name, d);
    }
    for (unit_name, alternative) in &place.run_units {
        let Some(unit) = unit_named(stack, unit_name, d) else {
            continue;
        };
        if unit.variants.is_empty() {
            continue;
        }
        if !unit.has_alternatives() {
            d.error(
                unit_name.loc,
                format!(
                    "unit `{}` has no alternatives to choose between",
                    unit.name.name
                ),
            );
        } else if unit.alternative(&alternative.name).is_none() {
            d.error(
                alternative.loc,
                format!(
                    "unit `{}` has no alternative `{}`; its alternatives are {}",
                    unit.name.name,
                    alternative.name,
                    unit.alternative_names()
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
    }
    for (name, value) in &place.settings {
        let Some(decl) = stack.setting(&name.name) else {
            d.error(
                name.loc,
                format!(
                    "no setting `{}` is declared in tay.yaml, so it cannot be set",
                    name.name
                ),
            );
            continue;
        };
        references(stack, value.expr(), None, d);
        // A `--set` is text until its setting's type is applied, when it is resolved.
        if let (SettingValue::Written(expr), Some(ty)) = (value, decl.ty)
            && let Some(written) = expr.written_type()
            && written != ty
        {
            d.push(
                Diagnostic::new(
                    expr.loc,
                    format!(
                        "setting `{}` is {}; this value is {}",
                        name.name,
                        ty.describe(),
                        written.describe()
                    ),
                )
                .with(decl.name.loc, "the setting is declared here"),
            );
        }
    }
}
