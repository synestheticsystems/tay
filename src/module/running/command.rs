//! `command`: something run once, on the host, inside another unit's
//! container, or in a container of its own made for the run.

use crate::diagnostic::Diagnostic;
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::Node;

use super::common::{self, environment, run_form};
use crate::module::shape::{Given, GivenValue, Place, RestartDefault, Runs, Slot, UnitShape};
use crate::module::want::Want;
use crate::module::{Cx, Module};

pub struct Command;

const FIELDS: &[&str] = &[
    "run",
    "check",
    "undo",
    "in",
    "image",
    "mounts",
    "user",
    "workdir",
    "env",
    "secret_env",
    "stdin",
    "produces",
    "outputs",
];

const FILE_PATH: Place = Place::Readable("a file path");

impl Module for Command {
    fn name(&self) -> &'static str {
        "command"
    }

    fn joined_by_entry(&self) -> &'static [&'static str] {
        &["env", "secret_env"]
    }

    fn check(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<UnitShape> {
        let Some(with) = with else {
            cx.d.error(cx.uses, "a `command` unit needs `with` and in it a `run`");
            return None;
        };
        let errors_before = cx.d.len();
        let d = &mut *cx.d;
        let settings = Fields::of(with, "the settings of a `command` unit", FIELDS, d)?;
        let mut slots = Vec::new();
        let mut production = Vec::new();
        if let Some(run) = settings.require("run", d) {
            slots.extend(run_form(run, "`run`", d).unwrap_or_default());
        }
        for form in ["check", "undo"] {
            if let Some(node) = settings.get(form) {
                slots.extend(run_form(node, &format!("`{form}`"), d).unwrap_or_default());
            }
        }
        let runs = match (settings.entry("in"), settings.entry("image")) {
            (Some(_), Some((key, _))) => {
                d.error(
                    key.loc,
                    "a `command` unit runs in at most one of `in` and `image`; this one has both",
                );
                Runs::OnHost
            }
            (Some((_, unit)), None) => match fields::name(unit, "`in`", d) {
                Some(unit) => Runs::InsideUnit(unit),
                None => Runs::OnHost,
            },
            (None, Some((_, image))) => {
                slots.extend(Slot::read(
                    image,
                    Want::Text,
                    Place::Readable("an image name"),
                    d,
                ));
                Runs::OneOffContainer
            }
            (None, None) => Runs::OnHost,
        };
        let one_off = matches!(runs, Runs::OneOffContainer);
        for field in ["mounts", "user"] {
            if let Some((key, _)) = settings.entry(field).filter(|_| !one_off) {
                d.error(
                    key.loc,
                    format!("`{field}` goes with `image`: it is for a command run in a container of its own"),
                );
            }
        }
        if one_off {
            if let Some(mounts) = settings.get("mounts") {
                let (mount_slots, concerns) = common::mounts(mounts, d);
                slots.extend(mount_slots);
                production.extend(concerns);
            }
            if let Some(user) = settings.get("user") {
                slots.extend(Slot::read(
                    user,
                    Want::Scalar,
                    Place::Readable("a container's settings"),
                    d,
                ));
            }
        }
        if let Some(workdir) = settings.get("workdir") {
            slots.extend(Slot::read(
                workdir,
                Want::Text,
                Place::Readable("a working folder"),
                d,
            ));
        }
        slots.extend(environment(&settings, d));
        if let Some(stdin) = settings.get("stdin") {
            slots.extend(Slot::read(stdin, Want::Text, FILE_PATH, d));
        }
        let mut gives = Vec::new();
        if let Some(produces) = settings.get("produces") {
            let names =
                fields::name_list(produces, "a produced value", names::value_name_problem, d);
            for name in unique(names, d) {
                gives.push(Given {
                    name,
                    value: GivenValue::Produced,
                });
            }
        }
        let mut outputs = Vec::new();
        if let Some(node) = settings.get("outputs") {
            for (name, path) in fields::keyed(node, "an output", names::value_name_problem, d) {
                slots.extend(Slot::read(path, Want::Text, FILE_PATH, d));
                outputs.push(Given {
                    name,
                    value: GivenValue::Output,
                });
            }
        }
        (cx.d.len() == errors_before).then_some(UnitShape {
            runs,
            slots,
            gives,
            outputs,
            production,
            ready_without_check: "when it exits with 0",
            restart_by_default: RestartDefault::NEVER,
        })
    }
}

fn unique(names: Vec<NameAt>, d: &mut crate::diagnostic::Diagnostics) -> Vec<NameAt> {
    let mut kept: Vec<NameAt> = Vec::new();
    for name in names {
        match kept.iter().find(|k| k.name == name.name) {
            Some(first) => d.push(
                Diagnostic::new(name.loc, format!("`{}` is listed twice", name.name))
                    .with(first.loc, "the first one is here"),
            ),
            None => kept.push(name),
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use crate::module::testing::check;
    use crate::module::{GivenValue, Runs};

    #[test]
    fn where_a_command_runs() {
        let runs = |with: &str| check("command", with).0.unwrap().runs;
        assert!(matches!(runs("run: [true]\n"), Runs::OnHost));
        assert!(
            matches!(runs("run: [true]\nin: database\n"), Runs::InsideUnit(u) if u.name == "database")
        );
        assert!(matches!(
            runs("run: [true]\nimage: alpine\n"),
            Runs::OneOffContainer
        ));
    }

    #[test]
    fn run_takes_a_list_or_a_shell_and_mounts_need_an_image() {
        let (_, errors) = check(
            "command",
            "run: { shell: \"a | b\" }\ncheck: \"test -f x\"\n",
        );
        assert_eq!(
            errors,
            ["2:8: `check` must be a list of the program and its arguments; this is text"]
        );
        let (_, errors) = check("command", "run: [x]\nmounts: [{ from: a, to: /a }]\n");
        assert_eq!(
            errors,
            ["2:1: `mounts` goes with `image`: it is for a command run in a container of its own"]
        );
        let (_, errors) = check("command", "run: [x]\nin: db\nimage: alpine\n");
        assert_eq!(
            errors,
            ["3:1: a `command` unit runs in at most one of `in` and `image`; this one has both"]
        );
    }

    #[test]
    fn produced_values_and_outputs_are_given() {
        let (shape, errors) = check(
            "command",
            "run: [build]\nproduces: [token]\noutputs: { server: target/debug/server }\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
        let shape = shape.unwrap();
        assert!(matches!(shape.gives[0].value, GivenValue::Produced));
        assert_eq!(shape.outputs[0].name.name, "server");
        let (_, errors) = check("command", "run: [x]\nproduces: [a, a]\n");
        assert_eq!(errors, ["2:15: `a` is listed twice"]);
    }

    #[test]
    fn a_literal_in_secret_env_is_an_error() {
        let (_, errors) = check("command", "run: [x]\nsecret_env: { TOKEN: hunter2 }\n");
        assert_eq!(
            errors,
            [
                "2:22: `secret_env` takes only a reference to a secret value; a secret written into a definition is not a secret"
            ]
        );
    }
}
