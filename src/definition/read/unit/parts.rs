//! The fields every unit has, whatever module it uses: the values it gives,
//! its readiness check, its restart policy and the folders it watches.

use crate::definition::{Check, Endpoint, Give, GiveValue, Ready, Restart};
use crate::diagnostic::Diagnostics;
use crate::syntax::expr::{self, Expr};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names;
use crate::syntax::yaml::Node;

pub fn gives(node: &Node, d: &mut Diagnostics) -> Vec<Give> {
    fields::keyed(node, "a value's name", names::value_name_problem, d)
        .into_iter()
        .filter_map(|(name, value)| {
            if name.name == "outputs" {
                d.error(
                    name.loc,
                    "`outputs` cannot be a value's name: `${unit.outputs.name}` names a file a unit builds",
                );
                return None;
            }
            let value = if value.as_map().is_some() {
                GiveValue::Endpoint(Box::new(endpoint(value, d)?))
            } else {
                GiveValue::Value(expr::from_node(value, d)?)
            };
            Some(Give { name, value })
        })
        .collect()
}

fn endpoint(node: &Node, d: &mut Diagnostics) -> Option<Endpoint> {
    let fields = Fields::of(
        node,
        "an endpoint (a value written as a mapping)",
        &["port", "scheme", "path"],
        d,
    )?;
    let port = fields
        .require("port", d)
        .and_then(|n| expr::from_node(n, d));
    let scheme = fields.get("scheme").and_then(|n| expr::from_node(n, d));
    let path = fields.get("path").and_then(|n| expr::from_node(n, d));
    if let Some(path) = &path
        && let Some(text) = path.literal_text()
        && !text.starts_with('/')
    {
        d.error(path.loc, "an endpoint's `path` starts with `/`");
    }
    Some(Endpoint {
        loc: node.loc,
        port: port?,
        scheme,
        path,
    })
}

pub fn ready(node: &Node, d: &mut Diagnostics) -> Option<Ready> {
    let fields = Fields::of(
        node,
        "`ready`",
        &["http", "tcp", "command", "log", "within"],
        d,
    )?;
    let checks: Vec<(&str, &Node)> = ["http", "tcp", "command", "log"]
        .into_iter()
        .filter_map(|kind| fields.entry(kind).map(|(key, _)| (kind, key)))
        .collect();
    if let [_, (_, second), ..] = checks.as_slice() {
        d.error(
            second.loc,
            "`ready` has one check: `http`, `tcp`, `command` or `log`",
        );
        return None;
    }
    let Some((kind, _)) = checks.first() else {
        d.error(
            fields.loc(),
            "`ready` needs a check: `http`, `tcp`, `command` or `log`",
        );
        return None;
    };
    let value = fields.get(kind)?;
    let check = match *kind {
        "http" => Check::Http(expr::from_node(value, d)?),
        "tcp" => Check::Tcp(expr::from_node(value, d)?),
        "log" => Check::Log(expr::from_node(value, d)?),
        _ => Check::Command(fields::value_list(value, "a readiness `command`", d)?),
    };
    let within = fields
        .require("within", d)
        .and_then(|n| expr::from_node(n, d))?;
    Some(Ready { check, within })
}

pub fn restart(node: &Node, d: &mut Diagnostics) -> Option<Restart> {
    let fields = Fields::of(node, "`restart`", &["on", "attempts"], d)?;
    Some(Restart {
        on: fields.get("on").and_then(|n| expr::from_node(n, d)),
        attempts: fields.get("attempts").and_then(|n| expr::from_node(n, d)),
    })
}

pub fn watch(node: &Node, d: &mut Diagnostics) -> Vec<Expr> {
    fields::value_list(node, "`watch`", d).unwrap_or_default()
}
