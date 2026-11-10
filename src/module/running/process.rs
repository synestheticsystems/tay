//! `process`: a command run on the host.

use crate::syntax::fields::{self, Fields};
use crate::syntax::names;
use crate::syntax::yaml::Node;
use crate::value::Value;

use super::common::{Variables, argv, argv_list, variables};
use crate::module::description::{Description, RestartDefault, Runs, UnitKind};
use crate::module::settings::{Destination, Settings, Slot, WrittenSettings, each, maybe};
use crate::module::want::{PathUse, Want};
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

/// A process's settings, each value a [`Slot`] as written or a [`Value`]
/// once resolved.
#[derive(Debug)]
struct ProcessSettings<V> {
    command: Vec<V>,
    /// `None`: the stack's root.
    workdir: Option<V>,
    variables: Variables<V>,
    /// The variables passed through from the environment tay was started
    /// in. `None`: `PATH`, `HOME`, `USER`, `LANG` and `TMPDIR`.
    inherit: Option<Vec<String>>,
    /// Commands run to completion, in order, before the first start.
    prepare: Vec<Vec<V>>,
}

impl<V> ProcessSettings<V> {
    fn map<'a, W>(&'a self, f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<ProcessSettings<W>> {
        let command = each(&self.command, f);
        let workdir = maybe(&self.workdir, f);
        let variables = self.variables.map(f);
        let prepare: Vec<Option<Vec<W>>> =
            self.prepare.iter().map(|words| each(words, f)).collect();
        Some(ProcessSettings {
            command: command?,
            workdir: workdir?,
            variables: variables?,
            inherit: self.inherit.clone(),
            prepare: prepare.into_iter().collect::<Option<_>>()?,
        })
    }
}

impl WrittenSettings for ProcessSettings<Slot> {
    fn slots(&self) -> Vec<&Slot> {
        let mut slots = Vec::new();
        self.map(&mut |slot| {
            slots.push(slot);
            Some(())
        });
        slots
    }

    fn resolve(
        &self,
        resolve: &mut dyn FnMut(&Slot) -> Option<Value>,
    ) -> Option<Box<dyn Settings>> {
        Some(Box::new(self.map(resolve)?))
    }
}

impl Settings for ProcessSettings<Value> {}

impl Module for Process {
    fn name(&self) -> &'static str {
        "process"
    }

    fn joined_by_entry(&self) -> &'static [&'static str] {
        &["env", "secret_env"]
    }

    fn read(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<Description> {
        let settings = read(with, cx)?;
        Some(Description::new(
            UnitKind::Runs(Runs::OnHost),
            Box::new(settings),
            "when it has started",
            RestartDefault::OnFailure,
        ))
    }
}

fn read(with: Option<&Node>, cx: &mut Cx<'_>) -> Option<ProcessSettings<Slot>> {
    let Some(with) = with else {
        cx.d.error(
            cx.uses,
            "a `process` unit needs `with` and in it a `command`",
        );
        return None;
    };
    let errors_before = cx.d.len();
    let d = &mut *cx.d;
    let settings = Fields::of(with, "the settings of a `process` unit", FIELDS, d)?;
    let command = settings
        .require("command", d)
        .and_then(|command| argv(command, "`command`", d));
    let workdir = settings.get("workdir").and_then(|workdir| {
        Slot::read(
            workdir,
            Want::Path(PathUse::Named),
            Destination::Readable("a working folder"),
            d,
        )
    });
    let variables = variables(&settings, d);
    let inherit = settings.get("inherit").map(|inherit| {
        fields::name_list(
            inherit,
            "a variable to inherit",
            names::variable_name_problem,
            d,
        )
        .into_iter()
        .map(|name| name.name)
        .collect()
    });
    let prepare = settings
        .get("prepare")
        .and_then(|prepare| argv_list(prepare, "each command in `prepare`", d))
        .unwrap_or_default();
    if cx.d.len() != errors_before {
        return None;
    }
    Some(ProcessSettings {
        command: command?,
        workdir,
        variables,
        inherit,
        prepare,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::testing::{self, check};

    fn settings(with: &str) -> ProcessSettings<Slot> {
        let (settings, errors) = testing::read(with, |node, cx| read(Some(node), cx));
        assert!(errors.is_empty(), "{errors:?}");
        settings.unwrap()
    }

    #[test]
    fn a_command_is_required_and_must_be_a_list() {
        let (description, errors) = check("process", "workdir: web\n");
        assert!(description.is_none());
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
    fn a_process_runs_on_the_host_and_restarts_on_failure() {
        let description = check("process", "command: [a]\n").0.unwrap();
        assert!(matches!(description.kind, UnitKind::Runs(Runs::OnHost)));
        assert_eq!(description.restart_by_default, RestartDefault::OnFailure);
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
    fn each_setting_is_read_into_its_place_and_resolved_there() {
        let read = settings(
            "command: [serve, --port, 80]\nworkdir: ./app\nenv: { LOG: debug }\nsecret_env: { KEY: \"${key.value}\" }\ninherit: [PATH]\nprepare: [[bun, install], [bun, build]]\n",
        );
        assert_eq!(
            read.workdir.as_ref().map(|slot| slot.want),
            Some(Want::Path(PathUse::Named))
        );
        assert_eq!(read.slots().len(), 10);
        // Each value is resolved where it is written: here, to its own text.
        let resolved = read
            .map(&mut |slot| Some(Value::text(format!("{:?}", slot.expr.kind))))
            .unwrap();
        let text = |value: &Value| value.exact_text().unwrap();
        assert_eq!(resolved.command.len(), 3);
        assert!(text(&resolved.command[2]).contains("Number(80)"));
        assert_eq!(resolved.variables.env[0].0, "LOG");
        assert_eq!(resolved.variables.secret_env[0].0, "KEY");
        assert_eq!(resolved.inherit, Some(vec!["PATH".to_string()]));
        assert_eq!(resolved.prepare.len(), 2);
        assert!(text(&resolved.prepare[1][1]).contains("build"));
    }

    #[test]
    fn a_value_that_cannot_be_resolved_leaves_no_settings_and_every_value_is_tried() {
        let read = settings("command: [a, b, c]\n");
        let mut tried = 0;
        let resolved = read.map(&mut |_| {
            tried += 1;
            (tried != 2).then(|| Value::text("x"))
        });
        assert!(resolved.is_none());
        assert_eq!(tried, 3);
    }
}
