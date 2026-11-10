//! `process`: a command run on the host.

use crate::syntax::fields::{self, Fields};
use crate::syntax::names;
use crate::syntax::yaml::Node;

use super::common::{argv, argv_list, environment};
use crate::module::shape::{Place, RestartDefault, Runs, Slot, UnitKind, UnitShape};
use crate::module::want::Want;
use crate::module::{Cx, Module};

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

    fn joined_by_entry(&self) -> &'static [&'static str] {
        &["env", "secret_env"]
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
        let mut shape = UnitShape::new(
            UnitKind::Runs(Runs::OnHost),
            "when it has started",
            RestartDefault::OnFailure,
        );
        let slots = &mut shape.slots;
        if let Some(command) = settings.require("command", cx.d) {
            slots.extend(argv(command, "`command`", cx.d).unwrap_or_default());
        }
        if let Some(workdir) = settings.get("workdir") {
            slots.extend(Slot::read(
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
        if let Some(prepare) = settings.get("prepare") {
            slots.extend(argv_list(prepare, "each command in `prepare`", cx.d).unwrap_or_default());
        }
        (cx.d.len() == errors_before).then_some(shape)
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
    fn preparation_commands_are_values_the_unit_uses_on_the_host() {
        let (shape, errors) = check("process", "command: [a]\nprepare: [[bun, install]]\n");
        assert!(errors.is_empty(), "{errors:?}");
        let shape = shape.unwrap();
        assert!(matches!(
            shape.kind,
            crate::module::UnitKind::Runs(crate::module::Runs::OnHost)
        ));
        assert_eq!(shape.slots.len(), 3);
    }
}
