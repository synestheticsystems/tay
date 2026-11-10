//! Templates: unit shapes with holes, filled by `${params.name}`.
//!
//! A template's unit is checked on its own for what does not depend on its
//! parameters: its fields, and that each `${params.x}` names a parameter it
//! declares. Each unit made from it is the template's unit with the
//! parameters put in place on the parsed structure, before anything else is
//! read from it, since a parameter may be a whole mapping that a way's
//! setting joins with; it is then read like any other unit. An error in the
//! template's own text found that way names the unit it was found for.

use std::collections::HashMap;

use crate::definition::{Body, Mechanism, Unit};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::syntax::expr::{self, Expr, Reference, Target};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::{Kind, Node, Scalar};

use super::{self as unit, UNIT_FIELDS};

mod substitute;

pub struct Template {
    pub name: NameAt,
    pub params: Vec<Param>,
    pub unit: Node,
    /// Whether the template's own checks found no errors. Units made from a
    /// template that has them are not read, so its errors are not repeated.
    pub readable: bool,
}

pub struct Param {
    pub name: NameAt,
    pub default: Option<Node>,
}

/// The fields of a unit, or of one of its ways, that references may not
/// fill, a parameter's among them.
const WRITTEN_OUT: &[&str] = &["uses", "from", "needs", "after", "action"];

/// Reads a template. A template whose own text has errors is still kept,
/// marked so, that the units made from it do not report it missing.
pub fn read(name: NameAt, node: &Node, d: &mut Diagnostics) -> Template {
    let errors_before = d.len();
    let Some(file) = Fields::of(node, "a template", &["params", "unit"], d) else {
        return Template {
            name,
            params: Vec::new(),
            readable: false,
            unit: node.clone(),
        };
    };
    let params: Vec<Param> = file
        .get("params")
        .map(|node| fields::keyed(node, "a parameter", names::value_name_problem, d))
        .unwrap_or_default()
        .into_iter()
        .map(|(name, decl)| {
            let default = Fields::of(decl, "a parameter's declaration", &["default"], d)
                .and_then(|decl| decl.get("default").cloned());
            Param { name, default }
        })
        .collect();
    let body = file.require("unit", d);
    if let Some(body) = body
        && let Some(fields) = Fields::of(body, "a template's unit", UNIT_FIELDS, d)
    {
        if let Some((key, _)) = fields.entry("from") {
            d.error(key.loc, "a template may not itself use `from`");
        }
        check_params(&fields, &params, d);
    }
    Template {
        name,
        params,
        readable: d.len() == errors_before,
        unit: body.unwrap_or(node).clone(),
    }
}

/// Every `${params.x}` in the template's unit names a declared parameter,
/// and none fills a field written out, in the unit or in one of its ways.
fn check_params(unit: &Fields<'_>, params: &[Param], d: &mut Diagnostics) {
    for (field, _) in unit.present() {
        let Some(value) = unit.get(field) else {
            continue;
        };
        match field {
            _ if WRITTEN_OUT.contains(&field) => refuse_params(field, value, d),
            "run" => {
                for way in value.as_map().into_iter().flatten() {
                    for entry in way.value.as_map().into_iter().flatten() {
                        match entry.key.as_str() {
                            Some(key) if WRITTEN_OUT.contains(&key.text.as_str()) => {
                                refuse_params(&key.text, &entry.value, d);
                            }
                            _ => declared_params(&entry.value, params, d),
                        }
                    }
                }
            }
            _ => declared_params(value, params, d),
        }
    }
}

/// Each reference in the strings of `node`, in order. A reference that does
/// not parse is reported.
fn references(node: &Node, d: &mut Diagnostics) -> Vec<Reference> {
    match &node.kind {
        Kind::Map(entries) => entries
            .iter()
            .flat_map(|entry| references(&entry.value, d))
            .collect(),
        Kind::Seq(items) => items.iter().flat_map(|item| references(item, d)).collect(),
        Kind::Scalar(Scalar::Str(s)) => {
            match expr::parse_string(&s.text, node.loc, s.content_column) {
                Ok(kind) => Expr {
                    loc: node.loc,
                    kind,
                }
                .references()
                .into_iter()
                .cloned()
                .collect(),
                Err(error) => {
                    d.push(error);
                    Vec::new()
                }
            }
        }
        Kind::Scalar(_) => Vec::new(),
    }
}

fn declared_params(node: &Node, params: &[Param], d: &mut Diagnostics) {
    for reference in references(node, d) {
        if let Target::Param(param) = &reference.target
            && !params.iter().any(|p| p.name.name == *param)
        {
            d.error(
                reference.loc,
                format!(
                    "`{}` names a parameter this template does not declare",
                    reference.written()
                ),
            );
        }
    }
}

fn refuse_params(field: &str, node: &Node, d: &mut Diagnostics) {
    for reference in references(node, d) {
        if matches!(reference.target, Target::Param(_)) {
            d.error(
                reference.loc,
                format!(
                    "`{}` cannot fill `{field}`: references are not allowed in `uses`, `from`, `needs`, `after` or `action`",
                    reference.written()
                ),
            );
        }
    }
}

/// Reads a unit made `from` a template.
pub fn made_from(
    name: NameAt,
    fields: &Fields<'_>,
    templates: &[Template],
    d: &mut Diagnostics,
) -> Unit {
    let errors_before = d.len();
    for (field, key) in fields.present() {
        if !matches!(field, "doc" | "from" | "params" | "needs" | "after") {
            d.error(
                key.loc,
                format!(
                    "a unit made `from` a template has only `doc`, `from`, `params`, `needs` and `after`; `{field}` belongs in the template"
                ),
            );
        }
    }
    let (from_key, from) = fields.entry("from").expect("the caller saw `from`");
    let Some(template_name) = fields::name(from, "`from`", d) else {
        return unit::unreadable(name);
    };
    let Some(template) = templates.iter().find(|t| t.name.name == template_name.name) else {
        let known: Vec<&str> = templates.iter().map(|t| t.name.name.as_str()).collect();
        d.error(
            from.loc,
            match known.as_slice() {
                [] => format!(
                    "there is no template `{}`; this stack has none",
                    template_name.name
                ),
                _ => format!(
                    "there is no template `{}`; the templates are {}",
                    template_name.name,
                    known.join(", ")
                ),
            },
        );
        return unit::unreadable(name);
    };
    let values = param_values(fields, template, from_key, d);
    let names = |key: &str, d: &mut Diagnostics| {
        fields
            .get(key)
            .map(|node| fields::name_list(node, &format!("`{key}`"), names::name_problem, d))
            .unwrap_or_default()
    };
    let (needs, after) = (names("needs", d), names("after", d));
    let doc = fields
        .get("doc")
        .and_then(|node| fields::text(node, "`doc`", d))
        .map(str::to_string);
    if !template.readable || d.len() != errors_before {
        return unit::unreadable(name);
    }
    // What goes wrong in the template's text is found for this unit, and
    // says so.
    let mut found = Diagnostics::default();
    let made = substitute::substitute(&template.unit, &values, &mut found).map(|body| {
        match Fields::of(&body, "a template's unit", UNIT_FIELDS, &mut found) {
            Some(body_fields) => unit::read_fields(name.clone(), &body_fields, &mut found),
            None => unit::unreadable(name.clone()),
        }
    });
    let mut made = made.unwrap_or_else(|| unit::unreadable(name.clone()));
    for error in found.into_sorted() {
        let in_template = error.loc.source == template.unit.loc.source;
        d.push(if in_template {
            error.with(
                from_key.loc,
                format!("for unit `{}`, made from the template here", name.name),
            )
        } else {
            error
        });
    }
    let join = |mechanism: &mut Mechanism| {
        mechanism.needs = unit::joined(&mechanism.needs, needs.clone());
        mechanism.after = unit::joined(&mechanism.after, after.clone());
    };
    match &mut made.body {
        Body::Group { needs: group_needs } => {
            *group_needs = unit::joined(group_needs, needs.clone());
            if let Some((key, _)) = fields.entry("after") {
                d.error(
                    key.loc,
                    "the template makes a group, which has only `doc` and `needs`; `after` does not belong here",
                );
                return unit::unreadable(name);
            }
        }
        Body::Module(mechanism) => join(mechanism),
        Body::Ways(ways) => ways.iter_mut().for_each(|way| join(&mut way.mechanism)),
        Body::Unreadable => {}
    }
    if doc.is_some() {
        made.doc = doc;
    }
    made
}

/// The value for each parameter: given by the unit, or the template's default.
fn param_values(
    fields: &Fields<'_>,
    template: &Template,
    from_key: &Node,
    d: &mut Diagnostics,
) -> HashMap<String, Node> {
    let mut values = HashMap::new();
    let given = fields
        .get("params")
        .map(|node| fields::keyed(node, "a parameter", names::value_name_problem, d))
        .unwrap_or_default();
    for (param, value) in given {
        if template.params.iter().any(|p| p.name.name == param.name) {
            values.insert(param.name, value.clone());
        } else {
            let declared: Vec<&str> = template
                .params
                .iter()
                .map(|p| p.name.name.as_str())
                .collect();
            d.push(
                Diagnostic::new(
                    param.loc,
                    format!(
                        "template `{}` has no parameter `{}`; its parameters are {}",
                        template.name.name,
                        param.name,
                        if declared.is_empty() {
                            "none".to_string()
                        } else {
                            declared.join(", ")
                        }
                    ),
                )
                .with(template.name.loc, "the template is here"),
            );
        }
    }
    for param in &template.params {
        if values.contains_key(&param.name.name) {
            continue;
        }
        match &param.default {
            Some(default) => {
                values.insert(param.name.name.clone(), default.clone());
            }
            None => d.push(
                Diagnostic::new(
                    from_key.loc,
                    format!(
                        "template `{}` needs the parameter `{}`, which has no default",
                        template.name.name, param.name.name
                    ),
                )
                .with(param.name.loc, "the parameter is declared here"),
            ),
        }
    }
    values
}
