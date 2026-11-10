//! Templates: unit shapes with holes, filled by `${params.name}`.
//!
//! A template's unit is checked once on its own, so an error in it is
//! reported once. Each unit made from it is then the template's unit with
//! the parameters put in place on the parsed structure, read like any other
//! unit; what goes wrong there depends on the values given.

use std::collections::HashMap;

use crate::definition::Unit;
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::syntax::expr::{self, ExprKind, Piece, Target};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::{Kind, Node, Scalar};

use super::{self as unit, UNIT_FIELDS};

mod substitute;

pub struct Template {
    pub name: NameAt,
    pub params: Vec<Param>,
    pub unit: Node,
    /// Whether the template's own unit reads without errors. Units made from
    /// a template that does not are not read, so its errors are not repeated.
    pub readable: bool,
}

pub struct Param {
    pub name: NameAt,
    pub default: Option<Node>,
}

pub fn read(name: NameAt, node: &Node, d: &mut Diagnostics) -> Option<Template> {
    let errors_before = d.len();
    let file = Fields::of(node, "a template", &["params", "unit"], d)?;
    let params: Vec<Param> = file
        .get("params")
        .map(|node| fields::keyed(node, "a parameter", names::value_name_problem, d))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, decl)| {
            let decl = Fields::of(decl, "a parameter's declaration", &["default"], d)?;
            let default = decl.get("default").cloned();
            if let Some(default) = &default {
                scalar(default, d)?;
            }
            Some(Param { name, default })
        })
        .collect();
    let body = file.require("unit", d)?.clone();
    let fields = Fields::of(&body, "a template's unit", UNIT_FIELDS, d)?;
    if let Some((key, _)) = fields.entry("from") {
        d.error(key.loc, "a template may not itself use `from`");
    }
    check_params(&body, &params, d);
    unit::read_fields(name.clone(), &fields, d);
    Some(Template {
        name,
        params,
        readable: d.len() == errors_before,
        unit: body,
    })
}

/// Every `${params.x}` in the template names a declared parameter.
fn check_params(node: &Node, params: &[Param], d: &mut Diagnostics) {
    match &node.kind {
        Kind::Map(entries) => entries
            .iter()
            .for_each(|e| check_params(&e.value, params, d)),
        Kind::Seq(items) => items.iter().for_each(|item| check_params(item, params, d)),
        Kind::Scalar(Scalar::Str(s)) => {
            let Ok(kind) = expr::parse_string(&s.text, node.loc, s.content_column) else {
                return;
            };
            let references = match &kind {
                ExprKind::Ref(reference) => vec![reference],
                ExprKind::Text(pieces) => pieces
                    .iter()
                    .filter_map(|p| match p {
                        Piece::Ref(reference) => Some(reference),
                        Piece::Lit(_) => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            for reference in references {
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
        Kind::Scalar(_) => {}
    }
}

fn scalar(node: &Node, d: &mut Diagnostics) -> Option<()> {
    match &node.kind {
        Kind::Scalar(Scalar::Str(_) | Scalar::Int(_) | Scalar::Bool(_)) => Some(()),
        _ => {
            d.error(
                node.loc,
                format!(
                    "a parameter's value is text, a whole number or true or false; this is {}",
                    node.describe()
                ),
            );
            None
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
    let Some(body) = substitute::substitute(&template.unit, &values, d) else {
        return unit::unreadable(name);
    };
    let mut found = Diagnostics::default();
    let mut made = match Fields::of(&body, "a template's unit", UNIT_FIELDS, &mut found) {
        Some(body_fields) => unit::read_fields(name.clone(), &body_fields, &mut found),
        None => unit::unreadable(name.clone()),
    };
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
    for way in &mut made.ways {
        way.needs = unit::joined(&way.needs, needs.clone());
        way.after = unit::joined(&way.after, after.clone());
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
            if scalar(value, d).is_some() {
                values.insert(param.name, value.clone());
            }
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
