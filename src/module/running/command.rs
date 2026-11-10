//! `command`: something run once, on the host, inside another unit's
//! container, or in a container of its own made for the run.

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::Node;
use crate::value::Value;

use super::common::{Mount, RunForm, Variables, mounts, run_form, variables};
use crate::module::description::{
    Description, ModuleGive, ModuleValue, Output, RestartDefault, Runs, UnitKind,
};
use crate::module::settings::{Destination, Settings, Slot, WrittenSettings, each_named, maybe};
use crate::module::want::{PathUse, Want};
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

const FILE_PATH: Destination = Destination::Readable("a file path");

/// A command's settings, each value a [`Slot`] as written or a [`Value`]
/// once resolved. Where it runs, inside another unit's container (`in`) or
/// not, is in the unit's description.
#[derive(Debug)]
struct CommandSettings<V> {
    run: RunForm<V>,
    /// Exit 0: already done. Exit 1: not done. `None`: run every time.
    check: Option<RunForm<V>>,
    undo: Option<RunForm<V>>,
    when_removed: WhenRemoved,
    /// The image of the container made for the run, with `mounts`.
    image: Option<V>,
    mounts: Vec<Mount<V>>,
    /// The user it runs as in a container.
    user: Option<V>,
    /// `None`: the stack's root on the host, the container's own in one.
    workdir: Option<V>,
    variables: Variables<V>,
    /// A file fed to `run` on standard input.
    stdin: Option<V>,
    /// The values the command writes out, each with whether it is secret.
    produces: Vec<(NameAt, bool)>,
    /// The files the command builds, by name.
    outputs: Vec<(NameAt, V)>,
}

/// What wiping a command that leaves the plan does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WhenRemoved {
    Keep,
    Undo,
}

impl<V> CommandSettings<V> {
    fn map<'a, W>(&'a self, f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<CommandSettings<W>> {
        let run = self.run.map(f);
        let check = maybe(&self.check, &mut |form| form.map(f));
        let undo = maybe(&self.undo, &mut |form| form.map(f));
        let image = maybe(&self.image, f);
        let mounts: Vec<Option<Mount<W>>> = self.mounts.iter().map(|m| m.map(f)).collect();
        let user = maybe(&self.user, f);
        let workdir = maybe(&self.workdir, f);
        let variables = self.variables.map(f);
        let stdin = maybe(&self.stdin, f);
        let outputs = each_named(&self.outputs, f);
        Some(CommandSettings {
            run: run?,
            check: check?,
            undo: undo?,
            when_removed: self.when_removed,
            image: image?,
            mounts: mounts.into_iter().collect::<Option<_>>()?,
            user: user?,
            workdir: workdir?,
            variables: variables?,
            stdin: stdin?,
            produces: self.produces.clone(),
            outputs: outputs?,
        })
    }
}

impl WrittenSettings for CommandSettings<Slot> {
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

impl Settings for CommandSettings<Value> {}

impl Module for Command {
    fn name(&self) -> &'static str {
        "command"
    }

    fn joined_by_entry(&self) -> &'static [&'static str] {
        &["env", "secret_env"]
    }

    fn read(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<Description> {
        let (settings, runs) = read(with, cx)?;
        let gives = settings
            .produces
            .iter()
            .map(|(name, secret)| ModuleGive {
                name: name.clone(),
                value: ModuleValue::Produced { secret: *secret },
            })
            .collect();
        let outputs = settings
            .outputs
            .iter()
            .map(|(name, path)| Output {
                name: name.clone(),
                path: path.expr.clone(),
            })
            .collect();
        Some(Description {
            gives,
            outputs,
            ..Description::new(
                UnitKind::Runs(runs),
                Box::new(settings),
                "when it exits with 0",
                RestartDefault::Never,
            )
        })
    }
}

fn read(with: Option<&Node>, cx: &mut Cx<'_>) -> Option<(CommandSettings<Slot>, Runs)> {
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
    let image = settings
        .get("image")
        .and_then(|image| Slot::read(image, Want::Text, Destination::Readable("an image name"), d));
    let run = settings
        .require("run", d)
        .and_then(|run| run_form(run, "`run`", d));
    let [check, undo] = ["check", "undo"].map(|form| {
        settings
            .get(form)
            .and_then(|node| run_form(node, &format!("`{form}`"), d))
    });
    let when_removed = match settings.get("when_removed") {
        None => WhenRemoved::Keep,
        Some(when_removed) => match fields::text(when_removed, "`when_removed`", d) {
            Some("keep") | None => WhenRemoved::Keep,
            Some("undo") => {
                if settings.get("undo").is_none() {
                    d.error(
                        when_removed.loc,
                        "`when_removed: undo` runs `undo`, and this command has none",
                    );
                }
                WhenRemoved::Undo
            }
            Some(other) => {
                d.error(
                    when_removed.loc,
                    format!("`when_removed` is `keep` or `undo`; this is `{other}`"),
                );
                WhenRemoved::Keep
            }
        },
    };
    let mounts = match settings.entry("mounts") {
        Some((_, node)) if one_off => mounts(node, d),
        Some((key, _)) => {
            d.error(
                key.loc,
                "`mounts` goes with `image`: it is for a command run in a container of its own",
            );
            Vec::new()
        }
        None => Vec::new(),
    };
    let user = match settings.entry("user") {
        Some((_, user)) if in_container => Slot::read(
            user,
            Want::Scalar,
            Destination::Readable("a container's settings"),
            d,
        ),
        Some((key, _)) => {
            d.error(
                key.loc,
                "`user` goes with `in` or `image`: it is the user a command runs as in a container",
            );
            None
        }
        None => None,
    };
    let workdir = settings.get("workdir").and_then(|workdir| {
        // In a container, the working folder is one of the container's.
        let want = match in_container {
            true => Want::Text,
            false => Want::Path(PathUse::Named),
        };
        Slot::read(workdir, want, Destination::Readable("a working folder"), d)
    });
    let variables = variables(&settings, d);
    let stdin = settings
        .get("stdin")
        .and_then(|stdin| Slot::read(stdin, Want::Path(PathUse::Named), FILE_PATH, d));
    let produces = settings
        .get("produces")
        .map(|produces| produced(produces, d))
        .unwrap_or_default();
    let outputs = settings
        .get("outputs")
        .map(|node| {
            fields::keyed(node, "an output", names::value_name_problem, d)
                .into_iter()
                .filter_map(|(name, path)| {
                    Some((
                        name,
                        Slot::read(path, Want::Path(PathUse::Named), FILE_PATH, d)?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    if cx.d.len() != errors_before {
        return None;
    }
    let settings = CommandSettings {
        run: run?,
        check,
        undo,
        when_removed,
        image,
        mounts,
        user,
        workdir,
        variables,
        stdin,
        produces,
        outputs,
    };
    Some((settings, runs))
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
/// `{ secret: true }` for a value that is a credential. Each name comes with
/// whether it is secret.
fn produced(node: &Node, d: &mut Diagnostics) -> Vec<(NameAt, bool)> {
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
    let mut produced: Vec<(NameAt, bool)> = Vec::new();
    for (name, secret) in named {
        match produced.iter().find(|(first, _)| first.name == name.name) {
            Some((first, _)) => d.push(
                Diagnostic::new(name.loc, format!("`{}` is listed twice", name.name))
                    .with(first.loc, "the first one is here"),
            ),
            None => produced.push((name, secret)),
        }
    }
    produced
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::testing::{self, check};

    fn settings(with: &str) -> CommandSettings<Slot> {
        let (read, errors) = testing::read(with, |node, cx| read(Some(node), cx));
        assert!(errors.is_empty(), "{errors:?}");
        read.unwrap().0
    }

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
        let read = settings("run: { shell: \"a | b\" }\ncheck: [test, -f, x]\n");
        assert!(matches!(read.run, RunForm::Shell(_)));
        assert!(matches!(&read.check, Some(RunForm::Argv(words)) if words.len() == 3));
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
        let read = settings("run: [x]\nimage: alpine\nmounts: [{ from: a, to: /a }]\n");
        assert_eq!((read.image.is_some(), read.mounts.len()), (true, 1));
    }

    #[test]
    fn the_working_folder_is_a_host_path_only_for_a_command_on_the_host_and_stdin_always() {
        let wants = |with: &str| {
            let read = settings(with);
            [read.workdir.unwrap().want, read.stdin.unwrap().want]
        };
        let path = Want::Path(PathUse::Named);
        assert_eq!(
            wants("run: [x]\nworkdir: app\nstdin: input\n"),
            [path, path]
        );
        assert_eq!(
            wants("run: [x]\nin: db\nworkdir: /app\nstdin: input\n"),
            [Want::Text, path]
        );
    }

    #[test]
    fn a_user_goes_with_a_container_inside_another_unit_or_of_its_own() {
        for with in [
            "run: [x]\nin: db\nuser: \"999\"\n",
            "run: [x]\nimage: a\nuser: app\n",
        ] {
            assert!(settings(with).user.is_some(), "{with}");
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
        let read = settings("run: [x]\nundo: [y]\nwhen_removed: undo\n");
        assert_eq!(read.when_removed, WhenRemoved::Undo);
        assert_eq!(settings("run: [x]\n").when_removed, WhenRemoved::Keep);
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
        let (description, errors) = check(
            "command",
            "run: [build]\nproduces: [token]\noutputs: { server: target/debug/server }\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
        let description = description.unwrap();
        assert!(matches!(
            description.gives[0].value,
            ModuleValue::Produced { secret: false }
        ));
        assert_eq!(description.outputs[0].name.name, "server");
        let read = settings("run: [issue]\nproduces: { token: { secret: true }, id: {} }\n");
        let secret: Vec<_> = read
            .produces
            .iter()
            .map(|(name, secret)| (name.name.as_str(), *secret))
            .collect();
        assert_eq!(secret, [("token", true), ("id", false)]);
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
        let read = settings("run: [x]\nsecret_env: { DSN: \"db://app:${pw.value}@db\" }\n");
        assert_eq!(
            read.variables.secret_env[0].1.destination,
            Destination::SecretEnv
        );
    }

    #[test]
    fn every_value_is_resolved_in_its_place() {
        let read = settings(
            "run: [a, b]\ncheck: { shell: c }\nundo: [d]\nimage: e\nmounts: [{ from: f, to: g }]\nuser: h\nworkdir: i\nenv: { J: j }\nstdin: k\noutputs: { l: m }\n",
        );
        let mut resolved = Vec::new();
        let settings = read
            .map(&mut |slot| {
                let text = slot.expr.literal_text()?;
                resolved.push(text.clone());
                Some(text)
            })
            .unwrap();
        assert_eq!(
            resolved,
            ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "m"]
        );
        assert_eq!(
            (
                settings.outputs[0].0.name.as_str(),
                settings.outputs[0].1.as_str()
            ),
            ("l", "m")
        );
    }
}
