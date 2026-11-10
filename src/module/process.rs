//! `process`: a command run on the host.

use crate::fields::{self, Fields};
use crate::names;
use crate::yaml::Node;

use super::common::{self, argv, argv_list, environment};
use super::shape::{Concern, Place, RestartDefault, Runs, UnitShape, Want};
use super::{Cx, Module};

pub struct Process;

const FIELDS: &[&str] = &[
    "command",
    "workdir",
    "env",
    "secret_env",
    "inherit",
    "prepare",
];

impl Module for Process {
    fn name(&self) -> &'static str {
        "process"
    }

    fn check(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<UnitShape> {
        let Some(with) = with else {
            cx.d.error(
                cx.uses,
                "a `process` unit needs `with` and in it a `command`",
            );
            return None;
        };
        let errors_before = cx.d.len();
        let settings = Fields::of(with, "the settings of a `process` unit", FIELDS, cx.d)?;
        let mut slots = Vec::new();
        let mut production = Vec::new();
        if let Some(command) = settings.require("command", cx.d) {
            slots.extend(argv(command, "`command`", cx.d).unwrap_or_default());
        }
        if let Some(workdir) = settings.get("workdir") {
            slots.extend(common::value(
                workdir,
                Want::Text,
                Place::Readable("a working folder"),
                cx.d,
            ));
        }
        slots.extend(environment(&settings, cx.d));
        if let Some(inherit) = settings.get("inherit") {
            fields::name_list(
                inherit,
                "a variable to inherit",
                names::variable_name_problem,
                cx.d,
            );
        }
        if let Some((key, prepare)) = settings.entry("prepare") {
            slots.extend(argv_list(prepare, "each command in `prepare`", cx.d).unwrap_or_default());
            if prepare
                .as_seq()
                .is_some_and(|commands| !commands.is_empty())
            {
                production.push(Concern::Prepares(key.loc));
            }
        }
        (cx.d.len() == errors_before).then_some(UnitShape {
            runs: Runs::OnHost,
            slots,
            gives: Vec::new(),
            outputs: Vec::new(),
            production,
            ready_without_check: "when it has started",
            restart_by_default: RestartDefault::ON_FAILURE,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::module::testing::check;

    #[test]
    fn a_command_is_required_and_must_be_a_list() {
        let (shape, errors) = check("process", "workdir: web\n");
        assert!(shape.is_none());
        assert_eq!(
            errors,
            ["1:1: `command` is required in the settings of a `process` unit"]
        );
        let (_, errors) = check("process", "command: \"node server.js\"\n");
        assert_eq!(
            errors,
            ["1:10: `command` must be a list of the program and its arguments; this is text"]
        );
    }

    #[test]
    fn unknown_settings_and_wrong_types_are_errors_at_their_place() {
        let (_, errors) = check(
            "process",
            "command: [node]\nworkdir: 5\ncolour: red\nenv: { 1X: a }\n",
        );
        assert_eq!(
            errors,
            [
                "2:10: this must be text; this is a whole number",
                "3:1: `colour` is not a field of the settings of a `process` unit; its fields are command, workdir, env, secret_env, inherit, prepare",
                "4:8: an environment variable: `1X` is not an environment variable name: letters, digits and underscores, not starting with a digit",
            ]
        );
    }

    #[test]
    fn preparation_is_a_production_concern_and_the_unit_runs_on_the_host() {
        let (shape, errors) = check("process", "command: [a]\nprepare: [[bun, install]]\n");
        assert!(errors.is_empty(), "{errors:?}");
        let shape = shape.unwrap();
        assert!(matches!(shape.runs, crate::module::Runs::OnHost));
        assert_eq!(shape.production.len(), 1);
        assert_eq!(shape.slots.len(), 3);
    }
}
