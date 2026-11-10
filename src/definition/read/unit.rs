//! Reading one unit: its form (one module, ways under `run`, a template, or a
//! group), and the ways the planner chooses between.

use std::borrow::Cow;

use crate::definition::{Action, Body, Give, GiveValue, Mechanism, Ready, Restart, Unit, Way};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::module::{self, Cx, Module, UnitKind};
use crate::syntax::expr::Expr;
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::Node;

pub mod template;
mod unit_fields;

use template::Template;

pub const UNIT_FIELDS: &[&str] = &[
    "doc", "uses", "with", "run", "from", "params", "needs", "after", "ready", "restart", "watch",
    "gives", "action", "args",
];

const WAY_FIELDS: &[&str] = &[
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
        action: None,
        body: Body::Unreadable,
    }
}

/// What a way is built from, as a unit writes it for all its ways or as one
/// way writes it for itself.
#[derive(Default)]
struct WayFields<'n> {
    with: Option<&'n Node>,
    needs: Vec<NameAt>,
    after: Vec<NameAt>,
    gives: Vec<Give>,
    ready: Option<Ready>,
    restart: Option<Restart>,
    watch: Option<Vec<Expr>>,
}

impl<'n> WayFields<'n> {
    fn read(fields: &Fields<'n>, d: &mut Diagnostics) -> WayFields<'n> {
        let names = |key: &str, d: &mut Diagnostics| {
            fields
                .get(key)
                .map(|node| fields::name_list(node, &format!("`{key}`"), names::name_problem, d))
                .unwrap_or_default()
        };
        WayFields {
            with: fields.get("with"),
            needs: names("needs", d),
            after: names("after", d),
            gives: fields
                .get("gives")
                .map(|node| unit_fields::gives(node, d))
                .unwrap_or_default(),
            ready: fields
                .get("ready")
                .and_then(|node| unit_fields::ready(node, d)),
            restart: fields
                .get("restart")
                .and_then(|node| unit_fields::restart(node, d)),
            watch: fields.get("watch").map(|node| unit_fields::watch(node, d)),
        }
    }
}

/// Reads a unit written out, or one a template makes, once the
/// parameters are in place.
pub fn read_fields(name: NameAt, fields: &Fields<'_>, d: &mut Diagnostics) -> Unit {
    let errors_before = d.len();
    let doc = fields
        .get("doc")
        .and_then(|node| fields::text(node, "`doc`", d))
        .map(str::to_string);
    let is_action = fields
        .get("action")
        .and_then(|node| fields::boolean(node, "`action`", d))
        .unwrap_or(false);
    let args = match fields.entry("args") {
        Some((_, args)) if is_action => unit_fields::args(args, d),
        Some((key, _)) => {
            d.error(
                key.loc,
                "`args` goes with `action: true`: only an action is asked for with arguments",
            );
            Vec::new()
        }
        None => Vec::new(),
    };
    let action = is_action.then_some(Action { args });
    if let Some((key, _)) = fields.entry("params") {
        d.error(key.loc, "`params` goes with `from`: they fill a template");
    }
    let body = match (fields.entry("uses"), fields.entry("run")) {
        (Some(_), Some((run, _))) => {
            d.error(
                run.loc,
                "a unit has one of `uses`, `run` and `from`, or none of them; this one has both `uses` and `run`",
            );
            None
        }
        (Some(_), None) => {
            let own = WayFields::read(fields, d);
            mechanism(&name.name, fields, own, &WayFields::default(), d)
                .map(|mechanism| Body::Module(Box::new(mechanism)))
        }
        (None, Some((_, run))) => ways(&name, fields, run, d).map(Body::Ways),
        (None, None) => Some(group(fields, d)),
    };
    match body {
        Some(body) if d.len() == errors_before => Unit {
            name,
            doc,
            action,
            body,
        },
        _ => unreadable(name),
    }
}

fn group(fields: &Fields<'_>, d: &mut Diagnostics) -> Body {
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
    Body::Group {
        needs: WayFields::read(fields, d).needs,
    }
}

fn ways(name: &NameAt, fields: &Fields<'_>, run: &Node, d: &mut Diagnostics) -> Option<Vec<Way>> {
    let shared = WayFields::read(fields, d);
    if let Some(with) = shared.with
        && with.as_map().is_none()
    {
        d.error(
            with.loc,
            format!(
                "`with` written for all of a unit's ways is a mapping of settings; this is {}",
                with.describe()
            ),
        );
    }
    let listed = fields::keyed(run, "a way", names::name_problem, d);
    if listed.is_empty() && run.as_map().is_some() {
        d.error(run.loc, "`run` lists no ways");
    }
    // Every way is read, so that the errors of each are reported.
    let ways: Vec<Option<Way>> = listed
        .into_iter()
        .map(|(way_name, node)| {
            let fields = Fields::of(node, "a way", WAY_FIELDS, d)?;
            let own = WayFields::read(&fields, d);
            let mechanism = mechanism(&name.name, &fields, own, &shared, d)?;
            Some(Way {
                name: way_name,
                mechanism,
            })
        })
        .collect();
    let ways: Vec<Way> = ways.into_iter().collect::<Option<_>>()?;
    same_value_names(name, &ways, d);
    Some(ways)
}

/// A module and its settings, with what a unit writes for all its ways
/// joined to what this way writes; for a unit with `uses`, `shared` is
/// empty. `fields` holds the `uses`.
fn mechanism(
    unit: &str,
    fields: &Fields<'_>,
    own: WayFields<'_>,
    shared: &WayFields<'_>,
    d: &mut Diagnostics,
) -> Option<Mechanism> {
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
    let with = joined_with(module, shared.with, own.with);
    let mut cx = Cx {
        unit,
        uses: uses_key.loc,
        d,
    };
    let mut description = module.read(with.as_deref(), &mut cx)?;
    if matches!(description.kind, UnitKind::Made) {
        for give in shared.gives.iter().chain(&own.gives) {
            if let GiveValue::Endpoint(endpoint) = &give.value
                && endpoint.host.is_none()
            {
                d.error(
                    endpoint.loc,
                    format!(
                        "unit `{unit}` is made and runs nothing that listens, so an endpoint it gives must write its `host`"
                    ),
                );
            }
        }
    }
    let mut gives = shared.gives.clone();
    let module_gives = std::mem::take(&mut description.gives)
        .into_iter()
        .map(|given| Give {
            name: given.name,
            value: GiveValue::Module(given.value),
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
    Some(Mechanism {
        module,
        uses: uses_key.loc,
        description,
        needs: joined(&shared.needs, own.needs),
        after: joined(&shared.after, own.after),
        gives,
        ready: own.ready.or_else(|| shared.ready.clone()),
        restart: own.restart.or_else(|| shared.restart.clone()),
        watch: own
            .watch
            .or_else(|| shared.watch.clone())
            .unwrap_or_default(),
    })
}

/// The settings a way's module reads: the way's own `with` laid over the
/// one its unit writes for all its ways, setting by setting, and entry by
/// entry for the settings the module says map names to values.
fn joined_with<'n>(
    module: &dyn Module,
    shared: Option<&'n Node>,
    own: Option<&'n Node>,
) -> Option<Cow<'n, Node>> {
    match (shared, own) {
        (Some(shared), Some(own)) => Some(Cow::Owned(shared.joined(own, module.joined_by_entry()))),
        (Some(only), None) | (None, Some(only)) => Some(Cow::Borrowed(only)),
        (None, None) => None,
    }
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

/// Every way gives the same value names, so that a unit using one
/// does not break when the unit is switched to another way of running.
fn same_value_names(unit: &NameAt, ways: &[Way], d: &mut Diagnostics) {
    let given = |way: &Way| -> Vec<NameAt> {
        let mechanism = &way.mechanism;
        let outputs = mechanism.outputs().iter().map(|output| NameAt {
            name: format!("outputs.{}", output.name.name),
            loc: output.name.loc,
        });
        mechanism
            .gives
            .iter()
            .map(|give| give.name.clone())
            .chain(outputs)
            .collect()
    };
    // Each name, with the first way that gives it and where.
    let mut every: Vec<(NameAt, &NameAt)> = Vec::new();
    for way in ways {
        for name in given(way) {
            if !every.iter().any(|(n, _)| n.name == name.name) {
                every.push((name, &way.name));
            }
        }
    }
    for way in ways {
        let this = &way.name;
        let names = given(way);
        for (name, that) in &every {
            if names.iter().any(|n| n.name == name.name) {
                continue;
            }
            d.push(
                Diagnostic::new(
                    this.loc,
                    format!(
                        "way `{}` of `{}` does not give `{}`, which way `{}` gives; every way gives the same values",
                        this.name, unit.name, name.name, that.name
                    ),
                )
                .with(name.loc, format!("`{}` gives it here", that.name)),
            );
        }
    }
}
