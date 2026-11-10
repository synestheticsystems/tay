//! Reading one unit: its form (one module, alternatives, a template, or a
//! group), and the variants the planner chooses between.

use crate::definition::{Give, GiveValue, ModuleUse, Unit, Variant};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::fields::{self, Fields};
use crate::module::{self, Cx};
use crate::names::{self, NameAt};
use crate::yaml::Node;

use super::template::{self, Template};
use super::unit_parts;

pub const UNIT_FIELDS: &[&str] = &[
    "doc", "uses", "with", "run", "from", "params", "needs", "after", "ready", "restart", "watch",
    "gives", "action",
];

const ALTERNATIVE_FIELDS: &[&str] = &[
    "uses", "with", "needs", "after", "ready", "restart", "watch", "gives",
];

pub fn read(name: NameAt, node: &Node, templates: &[Template], d: &mut Diagnostics) -> Unit {
    let Some(fields) = Fields::of(node, "a unit", UNIT_FIELDS, d) else {
        return unreadable(name);
    };
    if fields.get("from").is_some() {
        return template::made_from(name, &fields, templates, d);
    }
    read_fields(name, &fields, d)
}

/// A unit that other units may name but whose definition has errors.
pub fn unreadable(name: NameAt) -> Unit {
    Unit {
        name,
        doc: None,
        action: false,
        variants: Vec::new(),
    }
}

/// Needs, waits and values written on the unit itself, joined into each of its variants.
#[derive(Default)]
struct Shared {
    needs: Vec<NameAt>,
    after: Vec<NameAt>,
    gives: Vec<Give>,
}

impl Shared {
    fn read(fields: &Fields<'_>, d: &mut Diagnostics) -> Shared {
        let names = |key: &str, d: &mut Diagnostics| {
            fields
                .get(key)
                .map(|node| fields::name_list(node, &format!("`{key}`"), names::name_problem, d))
                .unwrap_or_default()
        };
        Shared {
            needs: names("needs", d),
            after: names("after", d),
            gives: fields
                .get("gives")
                .map(|node| unit_parts::gives(node, d))
                .unwrap_or_default(),
        }
    }
}

/// Reads a unit that is not made from a template, or a template's own unit.
pub fn read_fields(name: NameAt, fields: &Fields<'_>, d: &mut Diagnostics) -> Unit {
    let errors_before = d.len();
    let doc = fields
        .get("doc")
        .and_then(|node| fields::text(node, "`doc`", d))
        .map(str::to_string);
    let action = fields
        .get("action")
        .and_then(|node| fields::boolean(node, "`action`", d))
        .unwrap_or(false);
    if let Some((key, _)) = fields.entry("params") {
        d.error(key.loc, "`params` goes with `from`: they fill a template");
    }
    let shared = Shared::read(fields, d);
    let variants = match (fields.entry("uses"), fields.entry("run")) {
        (Some(_), Some((run, _))) => {
            d.error(
                run.loc,
                "a unit has one of `uses`, `run` and `from`, or none of them; this one has both `uses` and `run`",
            );
            None
        }
        (Some(_), None) => variant(&name.name, None, fields, Shared::default(), &shared, d)
            .map(|variant| vec![variant]),
        (None, Some((_, run))) => alternatives(&name, fields, run, &shared, d),
        (None, None) => group(fields, shared, d),
    };
    match variants {
        Some(variants) if d.len() == errors_before => Unit {
            name,
            doc,
            action,
            variants,
        },
        _ => unreadable(name),
    }
}

fn group(fields: &Fields<'_>, shared: Shared, d: &mut Diagnostics) -> Option<Vec<Variant>> {
    for (field, key) in fields.present() {
        if !matches!(field, "doc" | "needs") {
            d.error(
                key.loc,
                format!(
                    "a group (a unit with none of `uses`, `run` and `from`) has only `doc` and `needs`; `{field}` does not belong here"
                ),
            );
        }
    }
    Some(vec![Variant {
        alternative: None,
        module: None,
        needs: shared.needs,
        after: Vec::new(),
        gives: Vec::new(),
        outputs: Vec::new(),
        ready: None,
        restart: None,
        watch: Vec::new(),
    }])
}

fn alternatives(
    name: &NameAt,
    fields: &Fields<'_>,
    run: &Node,
    shared: &Shared,
    d: &mut Diagnostics,
) -> Option<Vec<Variant>> {
    for field in ["with", "ready", "restart", "watch"] {
        if let Some((key, _)) = fields.entry(field) {
            d.error(
                key.loc,
                format!("`{field}` goes in each alternative under `run`, not on the unit"),
            );
        }
    }
    let listed = fields::keyed(run, "an alternative", names::name_problem, d);
    if listed.is_empty() && run.as_map().is_some() {
        d.error(run.loc, "`run` lists no alternatives");
    }
    // Every alternative is read, so that the errors of each are reported.
    let variants: Vec<Option<Variant>> = listed
        .into_iter()
        .map(|(alternative, node)| {
            let own = Fields::of(node, "an alternative", ALTERNATIVE_FIELDS, d)?;
            let own_shared = Shared::read(&own, d);
            variant(&name.name, Some(alternative), &own, own_shared, shared, d)
        })
        .collect();
    let variants: Vec<Variant> = variants.into_iter().collect::<Option<_>>()?;
    same_value_names(name, &variants, d);
    Some(variants)
}

/// One way to run: a module and its settings, with what the unit writes for
/// all its ways joined to what this one writes.
fn variant(
    unit: &str,
    alternative: Option<NameAt>,
    fields: &Fields<'_>,
    own: Shared,
    shared: &Shared,
    d: &mut Diagnostics,
) -> Option<Variant> {
    let (uses_key, uses) = fields.entry("uses").or_else(|| {
        d.error(
            fields.loc(),
            format!("`uses` is required in {}", fields.what()),
        );
        None
    })?;
    let module_name = fields::text(uses, "`uses`", d)?;
    let Some(module) = module::find(module_name) else {
        d.error(
            uses.loc,
            format!(
                "`{module_name}` is not a module; the modules are {}",
                module::names().collect::<Vec<_>>().join(", ")
            ),
        );
        return None;
    };
    let mut cx = Cx {
        unit,
        uses: uses_key.loc,
        d,
    };
    let shape = module.check(fields.get("with"), &mut cx)?;
    let mut gives = shared.gives.clone();
    let module_gives = shape.gives.iter().map(|given| Give {
        name: given.name.clone(),
        value: GiveValue::Module(given.value.clone()),
    });
    for give in own.gives.into_iter().chain(module_gives) {
        match gives.iter().find(|g| g.name.name == give.name.name) {
            Some(first) => d.push(
                Diagnostic::new(
                    give.name.loc,
                    format!("`{}` is given twice", give.name.name),
                )
                .with(first.name.loc, "it is given here first"),
            ),
            None => gives.push(give),
        }
    }
    Some(Variant {
        alternative,
        outputs: shape
            .outputs
            .iter()
            .map(|given| given.name.clone())
            .collect(),
        module: Some(ModuleUse {
            module,
            uses: uses_key.loc,
            shape,
        }),
        needs: joined(&shared.needs, own.needs),
        after: joined(&shared.after, own.after),
        gives,
        ready: fields
            .get("ready")
            .and_then(|node| unit_parts::ready(node, d)),
        restart: fields
            .get("restart")
            .and_then(|node| unit_parts::restart(node, d)),
        watch: fields
            .get("watch")
            .map(|node| unit_parts::watch(node, d))
            .unwrap_or_default(),
    })
}

/// `first` and then those of `more` not already in it.
pub fn joined(first: &[NameAt], more: Vec<NameAt>) -> Vec<NameAt> {
    let mut all = first.to_vec();
    for name in more {
        if !all.iter().any(|n| n.name == name.name) {
            all.push(name);
        }
    }
    all
}

/// Every alternative gives the same value names, so that a unit using one
/// does not break when the unit is switched to another way of running.
fn same_value_names(unit: &NameAt, variants: &[Variant], d: &mut Diagnostics) {
    let given = |variant: &Variant| -> Vec<NameAt> {
        let outputs = variant.outputs.iter().map(|output| NameAt {
            name: format!("outputs.{}", output.name),
            loc: output.loc,
        });
        variant
            .gives
            .iter()
            .map(|give| give.name.clone())
            .chain(outputs)
            .collect()
    };
    // Each name, with the first alternative that gives it and where.
    let mut every: Vec<(NameAt, &NameAt)> = Vec::new();
    for variant in variants {
        let Some(alternative) = &variant.alternative else {
            continue;
        };
        for name in given(variant) {
            if !every.iter().any(|(n, _)| n.name == name.name) {
                every.push((name, alternative));
            }
        }
    }
    for variant in variants {
        let Some(this) = &variant.alternative else {
            continue;
        };
        let names = given(variant);
        for (name, that) in &every {
            if names.iter().any(|n| n.name == name.name) {
                continue;
            }
            d.push(
                Diagnostic::new(
                    this.loc,
                    format!(
                        "alternative `{}` of `{}` does not give `{}`, which alternative `{}` gives; every alternative gives the same values",
                        this.name, unit.name, name.name, that.name
                    ),
                )
                .with(name.loc, format!("`{}` gives it here", that.name)),
            );
        }
    }
}
