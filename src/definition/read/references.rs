//! Every name and reference in the definitions points at something that
//! exists: checked across all units and all their ways, whether or
//! not they come up, so a mistake shows before the unit is ever used.

use crate::definition::{Check, Choices, GiveValue, SettingValue, Stack, Unit, Way};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::module::{GivenValue, Runs, UnitKind, VolumeMount};
use crate::syntax::expr::{Expr, Part, Reference, Target, UnitRef, View};
use crate::syntax::names::NameAt;

pub fn check(stack: &Stack, places: &[&Choices], d: &mut Diagnostics) {
    for unit in &stack.units {
        for way in &unit.ways {
            for name in way.needs.iter().chain(&way.after) {
                unit_named(stack, name, d);
            }
            for mounted in way
                .module
                .iter()
                .flat_map(|module| &module.shape.volume_mounts)
            {
                volume_owned(stack, mounted, d);
            }
            if let Some(module) = &way.module
                && let UnitKind::Runs(Runs::InsideUnit(inner)) = &module.shape.kind
            {
                unit_named(stack, inner, d);
                if let Some(written) = way.needs.iter().find(|need| need.name == inner.name) {
                    d.push(
                        Diagnostic::new(
                            written.loc,
                            format!(
                                "`{}` is a need already, through `in`; it is not written again in `needs`",
                                inner.name
                            ),
                        )
                        .with(inner.loc, "`in` names it here"),
                    );
                }
            }
            for expr in exprs(way) {
                references(stack, expr, Some(unit), d);
            }
        }
    }
    for place in places {
        choices(stack, place, d);
    }
}

/// Every value written in a way.
fn exprs(way: &Way) -> Vec<&Expr> {
    let mut all: Vec<&Expr> = Vec::new();
    if let Some(module) = &way.module {
        all.extend(module.shape.slots.iter().map(|slot| &slot.expr));
        if let UnitKind::Runs(Runs::OwnContainer { published }) = &module.shape.kind {
            for entry in published {
                all.push(&entry.host_port.expr);
                all.extend(entry.address.iter().map(|slot| &slot.expr));
            }
        }
    }
    for give in &way.gives {
        match &give.value {
            GiveValue::Value(expr) | GiveValue::Module(GivenValue::Expr(expr)) => all.push(expr),
            GiveValue::Endpoint(endpoint) => {
                all.push(&endpoint.port);
                all.extend(endpoint.scheme.iter().chain(&endpoint.path));
            }
            GiveValue::Module(_) => {}
        }
    }
    if let Some(ready) = &way.ready {
        match &ready.check {
            Check::Http(expr) | Check::Tcp(expr) | Check::Log(expr) => all.push(expr),
            Check::Command(words) => all.extend(words),
        }
        all.push(&ready.within);
    }
    if let Some(restart) = &way.restart {
        all.extend(restart.on.iter().chain(&restart.attempts));
    }
    all.extend(&way.watch);
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
            unit,
            name,
            part,
            view,
        } => {
            view_unit(stack, view.as_ref(), reference, d);
            let Some(owner) = owner(unit, d) else { return };
            if owner.ways.is_empty() {
                return;
            }
            let gives: Vec<_> = owner.ways.iter().filter_map(|way| way.give(name)).collect();
            if gives.is_empty() {
                let mut names: Vec<&str> = owner
                    .ways
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
            } else if *part == Some(Part::Url)
                && gives
                    .iter()
                    .all(|g| matches!(&g.value, GiveValue::Endpoint(e) if e.scheme.is_none()))
            {
                d.error(
                    reference.loc,
                    format!("`{written}`: the endpoint has no `scheme`, so it has no URL"),
                );
            }
        }
        Target::Output { unit, name, view } => {
            view_unit(stack, view.as_ref(), reference, d);
            let Some(owner) = owner(unit, d) else { return };
            if !owner.ways.is_empty()
                && !owner
                    .ways
                    .iter()
                    .any(|way| way.outputs().iter().any(|output| output.name.name == *name))
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

/// A mounted volume belongs to a unit that owns a volume of that name, in
/// one of its ways.
fn volume_owned(stack: &Stack, mounted: &VolumeMount, d: &mut Diagnostics) {
    let Some(owner) = unit_named(stack, &mounted.unit, d) else {
        return;
    };
    if owner.ways.is_empty() {
        return;
    }
    let mut owned: Vec<&str> = owner
        .ways
        .iter()
        .filter_map(|way| way.module.as_ref())
        .flat_map(|module| module.shape.volumes.iter().map(|v| v.name.as_str()))
        .collect();
    if owned.contains(&mounted.volume.name.as_str()) {
        return;
    }
    owned.sort_unstable();
    owned.dedup();
    let list = match owned.as_slice() {
        [] => "it owns none".to_string(),
        names => format!(
            "its volumes are {}",
            names
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    d.error(
        mounted.volume.loc,
        format!(
            "unit `{}` owns no volume `{}`; {list}",
            owner.name.name, mounted.volume.name
        ),
    );
}

/// The unit a view names after `@` exists and runs in some way; under the
/// way it is chosen to run, the planner checks it again.
fn view_unit(stack: &Stack, view: Option<&View>, reference: &Reference, d: &mut Diagnostics) {
    let Some(View::Unit(name)) = view else {
        return;
    };
    let written = reference.written();
    let Some(unit) = stack.unit(name) else {
        d.error(
            reference.loc,
            format!("`{written}`: there is no unit `{name}`"),
        );
        return;
    };
    let runs = |way: &Way| {
        way.module
            .as_ref()
            .is_some_and(|module| matches!(module.shape.kind, UnitKind::Runs(_)))
    };
    if !unit.ways.is_empty() && !unit.ways.iter().any(runs) {
        d.error(
            reference.loc,
            format!(
                "`{written}` asks for the view of unit `{name}`, which runs nowhere; the unit named after `@` must be one that runs"
            ),
        );
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
    for (unit_name, way) in &place.run_units {
        let Some(unit) = unit_named(stack, unit_name, d) else {
            continue;
        };
        if unit.ways.is_empty() {
            continue;
        }
        if !unit.has_ways() {
            d.error(
                unit_name.loc,
                format!("unit `{}` has no ways to choose between", unit.name.name),
            );
        } else if unit.way(&way.name).is_none() {
            d.error(
                way.loc,
                format!(
                    "unit `{}` has no way `{}`; its ways are {}",
                    unit.name.name,
                    way.name,
                    unit.way_names()
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
        if value.typed(decl.ty).is_some() {
            continue;
        }
        // The message does not quote the value: it may be a secret.
        let message = match value {
            SettingValue::Written(written) => format!(
                "setting `{}` is {}; this value is {}",
                name.name,
                decl.ty.describe(),
                written.value.ty().describe()
            ),
            SettingValue::Argument { .. } => format!(
                "setting `{}` is {}, and this value is not",
                name.name,
                decl.ty.describe()
            ),
        };
        d.push(
            Diagnostic::new(value.loc(), message)
                .with(decl.name.loc, "the setting is declared here"),
        );
    }
}
