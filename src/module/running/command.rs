//! `command`: something run once, on the host, inside another unit's
//! container, or in a container of its own made for the run.

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::Node;

use super::common::{self, environment, run_form};
use crate::module::shape::{
    Given, GivenValue, Place, RestartDefault, Runs, Slot, UnitKind, UnitShape,
};
use crate::module::want::Want;
use crate::module::{Cx, Module};

pub struct Command;

const FIELDS: &[&str] = &[
    "run",
    "check",
    "undo",
    "when_removed",
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
        let runs = runs(&settings, d);
        let in_container = !matches!(runs, Runs::OnHost);
        let one_off = matches!(runs, Runs::OneOffContainer);
        let mut shape = UnitShape::new(
            UnitKind::Runs(runs),
            "when it exits with 0",
            RestartDefault::Never,
        );
        if let Some(image) = settings.get("image") {
            shape.slots.extend(Slot::read(
                image,
                Want::Text,
                Place::Readable("an image name"),
                d,
            ));
        }
        if let Some(run) = settings.require("run", d) {
            shape
                .slots
                .extend(run_form(run, "`run`", d).unwrap_or_default());
        }
        for form in ["check", "undo"] {
            if let Some(node) = settings.get(form) {
                shape
                    .slots
                    .extend(run_form(node, &format!("`{form}`"), d).unwrap_or_default());
            }
        }
        if let Some(when_removed) = settings.get("when_removed") {
            match fields::text(when_removed, "`when_removed`", d) {
                Some("keep") | None => {}
                Some("undo") if settings.get("undo").is_none() => d.error(
                    when_removed.loc,
                    "`when_removed: undo` runs `undo`, and this command has none",
                ),
                Some("undo") => {}
                Some(other) => d.error(
                    when_removed.loc,
                    format!("`when_removed` is `keep` or `undo`; this is `{other}`"),
                ),
            }
        }
        match settings.entry("mounts") {
            Some((_, mounts)) if one_off => common::mounts(mounts, &mut shape, d),
            Some((key, _)) => d.error(
                key.loc,
                "`mounts` goes with `image`: it is for a command run in a container of its own",
            ),
            None => {}
        }
        match settings.entry("user") {
            Some((_, user)) if in_container => shape.slots.extend(Slot::read(
                user,
                Want::Scalar,
                Place::Readable("a container's settings"),
                d,
            )),
            Some((key, _)) => d.error(
                key.loc,
                "`user` goes with `in` or `image`: it is the user a command runs as in a container",
            ),
            None => {}
        }
        if let Some(workdir) = settings.get("workdir") {
            shape.slots.extend(Slot::read(
                workdir,
                Want::Text,
                Place::Readable("a working folder"),
                d,
            ));
        }
        shape.slots.extend(environment(&settings, d));
        if let Some(stdin) = settings.get("stdin") {
            shape
                .slots
                .extend(Slot::read(stdin, Want::Text, FILE_PATH, d));
        }
        if let Some(produces) = settings.get("produces") {
            shape.gives.extend(produced(produces, d));
        }
        if let Some(node) = settings.get("outputs") {
            for (name, path) in fields::keyed(node, "an output", names::value_name_problem, d) {
                shape
                    .slots
                    .extend(Slot::read(path, Want::Text, FILE_PATH, d));
                shape.outputs.push(Given {
                    name,
                    value: GivenValue::Output,
                });
            }
        }
        (cx.d.len() == errors_before).then_some(shape)
    }
}

/// Where the command runs: inside the container `in` names, in a container
/// of its own from `image`, or on the host.
fn runs(settings: &Fields<'_>, d: &mut Diagnostics) -> Runs {
    match (settings.entry("in"), settings.entry("image")) {
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
        (None, Some(_)) => Runs::OneOffContainer,
        (None, None) => Runs::OnHost,
    }
}

/// `produces`: a list of names, or a mapping from a name to
/// `{ secret: true }` for a value that is a credential.
fn produced(node: &Node, d: &mut Diagnostics) -> Vec<Given> {
    const WHAT: &str = "a produced value";
    let named: Vec<(NameAt, bool)> = match node.as_map() {
        Some(_) => fields::keyed(node, WHAT, names::value_name_problem, d)
            .into_iter()
            .map(|(name, decl)| {
                let secret = Fields::of(decl, "a produced value's declaration", &["secret"], d)
                    .and_then(|decl| decl.get("secret"))
                    .and_then(|node| fields::boolean(node, "`secret`", d))
                    .unwrap_or(false);
                (name, secret)
            })
            .collect(),
        None => fields::name_list(node, WHAT, names::value_name_problem, d)
            .into_iter()
            .map(|name| (name, false))
            .collect(),
    };
    let mut gives: Vec<Given> = Vec::new();
    for (name, secret) in named {
        match gives.iter().find(|given| given.name.name == name.name) {
            Some(first) => d.push(
                Diagnostic::new(name.loc, format!("`{}` is listed twice", name.name))
                    .with(first.name.loc, "the first one is here"),
            ),
            None => gives.push(Given {
                name,
                value: GivenValue::Produced { secret },
            }),
        }
    }
    gives
}

#[cfg(test)]
mod tests {
    use crate::module::testing::check;
    use crate::module::{GivenValue, Runs, UnitKind};

    #[test]
    fn where_a_command_runs() {
        let runs = |with: &str| match check("command", with).0.unwrap().kind {
            UnitKind::Runs(runs) => runs,
            UnitKind::Made => panic!("a command runs"),
        };
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
        let (_, errors) = check(
            "command",
            "run: [x]\nin: db\nmounts: [{ from: a, to: /a }]\n",
        );
        assert_eq!(
            errors,
            ["3:1: `mounts` goes with `image`: it is for a command run in a container of its own"]
        );
        let (_, errors) = check("command", "run: [x]\nin: db\nimage: alpine\n");
        assert_eq!(
            errors,
            ["3:1: a `command` unit runs in at most one of `in` and `image`; this one has both"]
        );
    }

    #[test]
    fn a_user_goes_with_a_container_inside_another_unit_or_of_its_own() {
        for with in [
            "run: [x]\nin: db\nuser: \"999\"\n",
            "run: [x]\nimage: a\nuser: app\n",
        ] {
            let (_, errors) = check("command", with);
            assert!(errors.is_empty(), "{with}: {errors:?}");
        }
        let (_, errors) = check("command", "run: [x]\nuser: app\n");
        assert_eq!(
            errors,
            [
                "2:1: `user` goes with `in` or `image`: it is the user a command runs as in a container"
            ]
        );
    }

    #[test]
    fn when_removed_is_keep_or_undo_and_undo_needs_an_undo() {
        let (_, errors) = check("command", "run: [x]\nundo: [y]\nwhen_removed: undo\n");
        assert!(errors.is_empty(), "{errors:?}");
        let (_, errors) = check("command", "run: [x]\nwhen_removed: undo\n");
        assert_eq!(
            errors,
            ["2:15: `when_removed: undo` runs `undo`, and this command has none"]
        );
        let (_, errors) = check("command", "run: [x]\nwhen_removed: forget\n");
        assert_eq!(
            errors,
            ["2:15: `when_removed` is `keep` or `undo`; this is `forget`"]
        );
    }

    #[test]
    fn produced_values_are_a_list_or_a_mapping_that_marks_secrets() {
        let (shape, errors) = check(
            "command",
            "run: [build]\nproduces: [token]\noutputs: { server: target/debug/server }\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
        let shape = shape.unwrap();
        assert!(matches!(
            shape.gives[0].value,
            GivenValue::Produced { secret: false }
        ));
        assert_eq!(shape.outputs[0].name.name, "server");
        let (shape, errors) = check(
            "command",
            "run: [issue]\nproduces: { token: { secret: true }, id: {} }\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
        let secret: Vec<_> = shape
            .unwrap()
            .gives
            .iter()
            .map(|given| matches!(given.value, GivenValue::Produced { secret: true }))
            .collect();
        assert_eq!(secret, [true, false]);
        let (_, errors) = check("command", "run: [x]\nproduces: [a, a]\n");
        assert_eq!(errors, ["2:15: `a` is listed twice"]);
    }

    #[test]
    fn a_value_written_out_in_secret_env_is_an_error_and_one_built_around_a_reference_is_not() {
        let (_, errors) = check("command", "run: [x]\nsecret_env: { TOKEN: hunter2 }\n");
        assert_eq!(
            errors,
            [
                "2:22: a value in `secret_env` must have a secret in it, and this one is written out: a secret written into a definition is not a secret"
            ]
        );
        let (_, errors) = check(
            "command",
            "run: [x]\nsecret_env: { DSN: \"db://app:${pw.value}@db\" }\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
    }
}
