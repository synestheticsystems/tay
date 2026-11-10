//! The fields every unit has, whatever module it uses: the values it gives,
//! its readiness check, its restart policy and the folders it watches. Each
//! value's written form is checked here, whether or not the unit comes up.

use crate::definition::{Arg, Check, Endpoint, Give, GiveValue, Ready, Restart};
use crate::diagnostic::Diagnostics;
use crate::module::{self, Destination, PathUse, RestartOn, Slot, Want};
use crate::syntax::expr::{self, Expr, Piece};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names;
use crate::syntax::yaml::Node;

const READINESS: Destination = Destination::Readable("a readiness check");
const RESTART: Destination = Destination::Readable("a restart policy");
const WATCHED: Destination = Destination::Readable("a watched file or folder");

/// An action's `args`, declared as a template declares its parameters: by
/// name, each with a `default` or none.
pub fn args(node: &Node, d: &mut Diagnostics) -> Vec<Arg> {
    fields::keyed(node, "an argument", names::value_name_problem, d)
        .into_iter()
        .filter_map(|(name, decl)| {
            let decl = Fields::of(decl, "an argument's declaration", &["default"], d)?;
            let default = match decl.get("default") {
                Some(default) => Some(fields::literal(default, "an argument's default", d)?),
                None => None,
            };
            Some(Arg { name, default })
        })
        .collect()
}

pub fn gives(node: &Node, d: &mut Diagnostics) -> Vec<Give> {
    fields::keyed(node, "a value's name", names::value_name_problem, d)
        .into_iter()
        .filter_map(|(name, value)| {
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
        &["port", "scheme", "path", "host"],
        d,
    )?;
    let port = fields
        .require("port", d)
        .and_then(|n| Endpoint::PORT.read(n, d));
    let [scheme, path, host] = [
        ("scheme", Endpoint::SCHEME),
        ("path", Endpoint::PATH),
        ("host", Endpoint::HOST),
    ]
    .map(|(field, want)| optional(fields.get(field), want, d));
    if let Some(Some(path)) = &path
        && let expr::ExprKind::Text(pieces) = &path.kind
        && let Some(Piece::Lit(leading)) = pieces.first()
        && let Some(problem) = Endpoint::path_problem(leading)
    {
        d.error(path.loc, problem);
        return None;
    }
    Some(Endpoint {
        loc: node.loc,
        port: port?,
        scheme: scheme?,
        path: path?,
        host: host?,
    })
}

/// A field that may be absent, read where `want` is wanted: `Some(None)`
/// when it is absent, `None` when it is written wrongly.
fn optional(node: Option<&Node>, want: Want, d: &mut Diagnostics) -> Option<Option<Expr>> {
    match node {
        Some(node) => want.read(node, d).map(Some),
        None => Some(None),
    }
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
    let within = fields
        .require("within", d)
        .and_then(|n| Slot::read(n, Want::Duration, READINESS, d));
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
    let text = |d: &mut Diagnostics| Slot::read(value, Want::Text, READINESS, d);
    let check = match *kind {
        "http" => Check::Http(text(d)?),
        "tcp" => Check::Tcp(text(d)?),
        "log" => Check::Log(text(d)?),
        _ => Check::Command(module::argv(value, "a readiness `command`", READINESS, d)?),
    };
    Some(Ready {
        check,
        within: within?,
    })
}

pub fn restart(node: &Node, d: &mut Diagnostics) -> Option<Restart> {
    let fields = Fields::of(node, "`restart`", &["on", "attempts"], d)?;
    let [on, attempts] = [
        ("on", Want::OneOf(RestartOn::WORDS)),
        ("attempts", Want::Count),
    ]
    .map(|(field, want)| match fields.get(field) {
        Some(node) => Slot::read(node, want, RESTART, d).map(Some),
        None => Some(None),
    });
    Some(Restart {
        on: on?,
        attempts: attempts?,
    })
}

pub fn watch(node: &Node, d: &mut Diagnostics) -> Option<Vec<Slot>> {
    let items = fields::list(node, "`watch`", d)?;
    let watched: Vec<Option<Slot>> = items
        .iter()
        .map(|item| Slot::read(item, Want::Path(PathUse::Named), WATCHED, d))
        .collect();
    watched.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::source::Sources;
    use crate::syntax::yaml;

    /// Reads `text` with `read`, and gives what it read and each error as
    /// `line:column: message`.
    fn read<T>(text: &str, read: fn(&Node, &mut Diagnostics) -> T) -> (T, Vec<String>) {
        let mut sources = Sources::default();
        let id = sources.add_argument("unit");
        let mut d = Diagnostics::default();
        let node = yaml::parse(text, id, &mut d).expect("test YAML parses");
        let read = read(&node, &mut d);
        let errors = d
            .into_sorted()
            .iter()
            .map(|e| format!("{}:{}: {}", e.loc.line, e.loc.column, e.message))
            .collect();
        (read, errors)
    }

    #[test]
    fn ready_has_one_check_and_the_time_it_allows() {
        let (ready, errors) = read("{ log: listening, within: 1m }", super::ready);
        assert!(errors.is_empty(), "{errors:?}");
        let ready = ready.unwrap();
        assert!(matches!(ready.check, Check::Log(_)));
        assert_eq!(ready.within.want, Want::Duration);
        let (ready, errors) = read("{ command: [pg_isready, -q], within: 2m }", super::ready);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(matches!(ready.unwrap().check, Check::Command(words) if words.len() == 2));
        assert_eq!(
            read("{ command: [], within: 2m }", super::ready).1,
            ["1:12: a readiness `command` must name a program"]
        );
        assert_eq!(
            read("{ http: a, tcp: b, within: 1m }", super::ready).1,
            ["1:12: `ready` has one check: `http`, `tcp`, `command` or `log`"]
        );
        assert_eq!(
            read("{ within: 1m }", super::ready).1,
            ["1:1: `ready` needs a check: `http`, `tcp`, `command` or `log`"]
        );
        assert_eq!(
            read("{ tcp: a }", super::ready).1,
            ["1:1: `within` is required in `ready`"]
        );
        assert_eq!(
            read("{ tcp: a, within: 90 }", super::ready).1,
            ["1:19: this must be text; this is a whole number"]
        );
    }

    #[test]
    fn restart_is_on_failure_always_or_never_with_a_count_of_attempts() {
        for on in RestartOn::WORDS {
            let (restart, errors) = read(&format!("{{ on: {on}, attempts: 3 }}"), super::restart);
            assert!(errors.is_empty(), "{on}: {errors:?}");
            let restart = restart.unwrap();
            assert!(restart.on.is_some() && restart.attempts.is_some());
        }
        let (restart, errors) = read("{}", super::restart);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(restart.is_some_and(|r| r.on.is_none() && r.attempts.is_none()));
        assert_eq!(
            read("{ on: crash, attempts: 0 }", super::restart).1,
            [
                "1:7: this must be one of `failure`, `always`, `never`; this is `crash`",
                "1:24: this must be a whole number above 0; this is 0",
            ]
        );
    }

    #[test]
    fn an_endpoints_parts_are_checked_as_written() {
        let (gives, errors) = read(
            "{ api: { port: 8080, scheme: http, path: /v1, host: api.example.test } }",
            gives,
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert!(matches!(&gives[0].value, GiveValue::Endpoint(e) if e.path.is_some()));
        // A path built from a reference is checked once it is resolved.
        let (_, errors) = read(
            "{ api: { port: 80, path: \"${settings.p}\" } }",
            super::gives,
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            read("{ api: { port: 80, path: v1 } }", super::gives).1,
            ["1:26: an endpoint's `path` starts with `/`"]
        );
        assert_eq!(
            read("{ api: { scheme: http } }", super::gives).1,
            ["1:8: `port` is required in an endpoint (a value written as a mapping)"]
        );
    }

    #[test]
    fn a_watched_path_is_a_list_of_paths_on_the_host() {
        let (watch, errors) = read("[src, ./unit.yaml]", super::watch);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            watch
                .unwrap()
                .iter()
                .all(|slot| slot.want == Want::Path(PathUse::Named))
        );
        assert_eq!(
            read("src", super::watch).1,
            ["1:1: `watch` must be a list; this is text"]
        );
    }
}
