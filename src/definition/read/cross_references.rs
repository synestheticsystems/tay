//! Every name and reference in the definitions and the choices points at
//! something that exists: a need, a wait, a volume mounted, the container a
//! command runs in, a reference, and what a choice names. Checked across
//! all units and all their ways, whether or not they come up, so a mistake
//! shows before the unit is ever used; and across every profile, whichever
//! is chosen, so a mistake in the one a server uses shows wherever the
//! stack is planned.

use crate::definition::reference_errors;
use crate::definition::{
    Body, Check, Choices, GiveValue, Mechanism, Profile, SettingValue, Stack, Unit,
};
use crate::diagnostic::{Diagnostic, Diagnostics, quoted_list};
use crate::module::{ModuleValue, Runs, UnitKind, VolumeMount};
use crate::syntax::expr::{Expr, Part, Reference, Target, UnitRef, View};
use crate::syntax::names::NameAt;

/// Checks the stack's units, its own choices (its defaults and every
/// profile), and `outside`, the places of choices that are not written in
/// the stack.
pub fn check(stack: &Stack, outside: &[&Choices], d: &mut Diagnostics) {
    for unit in &stack.units {
        if let Body::Group { needs } = &unit.body {
            for name in needs {
                unit_named(stack, name, d);
            }
        }
        for mechanism in unit.mechanisms() {
            for name in mechanism.needs.iter().chain(&mechanism.after) {
                unit_named(stack, name, d);
            }
            for mounted in &mechanism.description.volume_mounts {
                volume_owned(stack, mounted, d);
            }
            if let UnitKind::Runs(Runs::InsideUnit(inner)) = &mechanism.description.kind {
                unit_named(stack, inner, d);
                if let Some(written) = mechanism.needs.iter().find(|need| need.name == inner.name) {
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
            for expr in exprs(mechanism) {
                references(stack, expr, Some(unit), d);
            }
        }
    }
    choices(stack, &stack.defaults, d);
    for profile in &stack.profiles {
        profile_names(stack, profile, d);
    }
    for place in outside {
        choices(stack, place, d);
    }
}

/// What `profile` names exists in the stack: its choices, and the settings
/// it requires. Whether a required setting is given a value is judged only
/// of the profile chosen, once every place is layered.
fn profile_names(stack: &Stack, profile: &Profile, d: &mut Diagnostics) {
    for required in &profile.require {
        if stack.setting(&required.name).is_none() {
            d.error(
                required.loc,
                format!(
                    "`require` names setting `{}`, which is not declared in tay.yaml",
                    required.name
                ),
            );
        }
    }
    choices(stack, &profile.choices, d);
}

/// Every value written in a mechanism.
fn exprs(mechanism: &Mechanism) -> Vec<&Expr> {
    let mut all: Vec<&Expr> = Vec::new();
    all.extend(
        mechanism
            .description
            .settings
            .slots()
            .into_iter()
            .map(|slot| &slot.expr),
    );
    for entry in &mechanism.description.publish {
        all.push(&entry.host_port.expr);
        all.extend(entry.address.iter().map(|slot| &slot.expr));
    }
    for give in &mechanism.gives {
        match &give.value {
            GiveValue::Value(expr) | GiveValue::Module(ModuleValue::Expr(expr)) => all.push(expr),
            GiveValue::Endpoint(endpoint) => {
                all.push(&endpoint.port);
                all.extend(
                    endpoint
                        .scheme
                        .iter()
                        .chain(&endpoint.path)
                        .chain(&endpoint.host),
                );
            }
            GiveValue::Module(_) => {}
        }
    }
    if let Some(ready) = &mechanism.ready {
        match &ready.check {
            Check::Http(slot) | Check::Tcp(slot) | Check::Log(slot) => all.push(&slot.expr),
            Check::Command(words) => all.extend(words.iter().map(|word| &word.expr)),
        }
        all.push(&ready.within.expr);
    }
    if let Some(restart) = &mechanism.restart {
        all.extend(
            restart
                .on
                .iter()
                .chain(&restart.attempts)
                .map(|slot| &slot.expr),
        );
    }
    all.extend(mechanism.watch.iter().map(|slot| &slot.expr));
    all
}

fn unit_named<'s>(stack: &'s Stack, name: &NameAt, d: &mut Diagnostics) -> Option<&'s Unit> {
    let unit = stack.unit(&name.name);
    if unit.is_none() {
        d.error(name.loc, format!("there is no unit `{}`", name.name));
    }
    unit
}

/// Every reference in `expr`, written in `this` unit if it is in one, names
/// something that exists and has what the reference asks for.
pub fn references(stack: &Stack, expr: &Expr, this: Option<&Unit>, d: &mut Diagnostics) {
    for reference in expr.references() {
        reference_target(stack, reference, this, d);
    }
}

pub fn reference_target(
    stack: &Stack,
    reference: &Reference,
    this: Option<&Unit>,
    d: &mut Diagnostics,
) {
    let owner = |unit: &UnitRef, d: &mut Diagnostics| match unit {
        UnitRef::This => {
            if this.is_none() {
                d.push(reference_errors::self_outside_a_unit(reference));
            }
            this
        }
        UnitRef::Named(name) => {
            let found = stack.unit(name);
            if found.is_none() {
                d.push(reference_errors::no_unit(reference, name));
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
            if matches!(owner.body, Body::Unreadable) {
                return;
            }
            let mechanisms = owner.mechanisms();
            let gives: Vec<_> = mechanisms.iter().filter_map(|m| m.give(name)).collect();
            if gives.is_empty() {
                let mut names: Vec<&str> = mechanisms
                    .iter()
                    .flat_map(|m| m.gives.iter().map(|g| g.name.name.as_str()))
                    .collect();
                names.sort_unstable();
                names.dedup();
                d.push(reference_errors::gives_no(
                    reference,
                    &owner.name.name,
                    name,
                    &names,
                ));
            } else if part.is_some()
                && !gives
                    .iter()
                    .any(|g| matches!(g.value, GiveValue::Endpoint(_)))
            {
                d.push(reference_errors::not_an_endpoint(reference, name));
            } else if *part == Some(Part::Url)
                && gives
                    .iter()
                    .all(|g| matches!(&g.value, GiveValue::Endpoint(e) if e.scheme.is_none()))
            {
                d.push(reference_errors::no_scheme(reference));
            }
        }
        Target::Output { unit, name, view } => {
            view_unit(stack, view.as_ref(), reference, d);
            let Some(owner) = owner(unit, d) else { return };
            if !matches!(owner.body, Body::Unreadable)
                && !owner.mechanisms().iter().any(|mechanism| {
                    mechanism
                        .outputs()
                        .iter()
                        .any(|output| output.name.name == *name)
                })
            {
                d.push(reference_errors::builds_no_output(
                    reference,
                    &owner.name.name,
                    name,
                ));
            }
        }
        Target::Setting(name) => {
            if stack.setting(name).is_none() {
                d.push(reference_errors::no_setting(reference, name));
            }
        }
        Target::Param(_) => d.push(reference_errors::param_outside_a_template(reference)),
        Target::Arg(name) => {
            let written = reference.written();
            match this.map(|unit| (unit, &unit.action)) {
                Some((_, Some(action))) if action.args.iter().any(|arg| arg.name.name == *name) => {
                }
                Some((unit, Some(_))) => d.error(
                    reference.loc,
                    format!(
                        "`{written}`: action `{}` declares no argument `{name}`",
                        unit.name.name
                    ),
                ),
                Some((unit, None)) => d.error(
                    reference.loc,
                    format!(
                        "`{written}` is an action's argument, and unit `{}` is not an action",
                        unit.name.name
                    ),
                ),
                None => d.error(
                    reference.loc,
                    format!("`{written}` is an action's argument, and this is not in a unit"),
                ),
            }
        }
        Target::Environment(_) | Target::HostListen => {}
    }
}

/// A mounted volume belongs to a unit that owns a volume of that name, in
/// one of its ways.
fn volume_owned(stack: &Stack, mounted: &VolumeMount, d: &mut Diagnostics) {
    let Some(owner) = unit_named(stack, &mounted.unit, d) else {
        return;
    };
    if matches!(owner.body, Body::Unreadable) {
        return;
    }
    let mut owned: Vec<&str> = owner
        .mechanisms()
        .into_iter()
        .flat_map(|mechanism| {
            mechanism
                .description
                .volumes
                .iter()
                .map(|v| v.name.as_str())
        })
        .collect();
    if owned.contains(&mounted.volume.name.as_str()) {
        return;
    }
    owned.sort_unstable();
    owned.dedup();
    let list = match owned.as_slice() {
        [] => "it owns none".to_string(),
        names => format!("its volumes are {}", quoted_list(names)),
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
    let Some(unit) = stack.unit(name) else {
        d.push(reference_errors::no_unit(reference, name));
        return;
    };
    let runs = |mechanism: &&Mechanism| matches!(mechanism.description.kind, UnitKind::Runs(_));
    if !matches!(unit.body, Body::Unreadable) && !unit.mechanisms().iter().any(runs) {
        d.push(reference_errors::view_of_a_unit_that_runs_nowhere(
            reference, name,
        ));
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
        match unit.body {
            Body::Unreadable => continue,
            Body::Ways(_) => {}
            Body::Group { .. } | Body::Module(_) => {
                d.error(
                    unit_name.loc,
                    format!("unit `{}` has no ways to choose between", unit.name.name),
                );
                continue;
            }
        }
        if unit.way(&way.name).is_none() {
            d.error(
                way.loc,
                format!(
                    "unit `{}` has no way `{}`; its ways are {}",
                    unit.name.name,
                    way.name,
                    quoted_list(unit.way_names())
                ),
            );
        }
    }
    for name in place.run_default.iter().flatten() {
        way_of_some_unit(stack, name, d);
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

/// A name in `run.default` is the name of a way some unit has: a misspelt
/// one would be passed over, and the next name in the list chosen in its
/// place. A unit whose definition has errors has no ways known, and has
/// been reported, so then nothing more is said.
fn way_of_some_unit(stack: &Stack, name: &NameAt, d: &mut Diagnostics) {
    let may_have_it =
        |unit: &Unit| matches!(unit.body, Body::Unreadable) || unit.way(&name.name).is_some();
    if stack.units.iter().any(may_have_it) {
        return;
    }
    let mut ways: Vec<&str> = stack.units.iter().flat_map(Unit::way_names).collect();
    ways.sort_unstable();
    ways.dedup();
    let list = match ways.as_slice() {
        [] => "no unit has ways at all".to_string(),
        ways => format!("the ways are {}", quoted_list(ways)),
    };
    d.error(
        name.loc,
        format!("no unit has a way `{}`; {list}", name.name),
    );
}
