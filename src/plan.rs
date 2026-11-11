//! The planner: from a stack's definitions and the environment's choices, the
//! units that come up, every value known before start, and the order units
//! could start in. It works from what modules say about units and never
//! asks which module a unit uses.

mod action;
mod assemble;
mod choices;
mod model;
mod order;
mod production;
mod reach;
mod resolve;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::definition::read::{self, Origin};
use crate::definition::{Profile, Stack};
use crate::diagnostic::source::{Loc, Sources};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::disk::{Asked, Asking};
use crate::module::HOST_LISTEN_BEFORE_START;
use crate::syntax::expr::Expr;
use crate::syntax::names::NameAt;

pub use action::ActionRun;
pub use model::{
    Because, Given, OpenedBy, Plan, PlannedMechanism, PlannedUnit, PublishedPort, Readiness, Seen,
    Unreachable, UsedFile,
};

use action::Arguments;
use choices::Layered;
use resolve::endpoint::LOOPBACK;
use resolve::{EnvironmentFacts, Resolver};

/// What `tay plan` is asked for, or `tay up`, which plans as it does.
pub struct Options {
    /// The profile, each `--set` in the order it applies, and the units
    /// named in place of `up`: what the environment remembers being asked
    /// for, and what this command adds, kept apart.
    pub asked: Asking,
    /// The home folder, where this person's own file of choices is.
    pub home: Option<PathBuf>,
    /// The folder that holds each stack's shared folder, when it is known.
    pub shared_folders: Option<PathBuf>,
    /// The action `tay run` runs and its arguments, which comes into the
    /// plan beside what `up` brings; `None` for every other command.
    pub action: Option<ActionRun>,
}

pub enum Outcome {
    Plan(Plan),
    /// The definitions have errors, which are in the diagnostics.
    Errors,
    /// This tay may not read the definitions; each line says why.
    TooOld(Vec<Diagnostic>),
}

/// What the planner works with while it walks the units that come up.
pub struct Planner<'s> {
    resolver: Resolver<'s>,
    root: PathBuf,
    production: bool,
    /// Each template file read so far, by path: its text, or why a
    /// reference in it cannot be parsed.
    template_files: HashMap<String, Result<Expr, Diagnostic>>,
}

/// Reads the stack at `root` and plans it.
pub fn plan(root: &Path, options: &Options, sources: &mut Sources, d: &mut Diagnostics) -> Outcome {
    let (stack, every_file_parsed) = match read::read(root, sources, d) {
        read::Outcome::Read {
            stack,
            every_file_parsed,
        } => (*stack, every_file_parsed),
        read::Outcome::TooOld(reasons) => return Outcome::TooOld(reasons),
        read::Outcome::Unreadable => return Outcome::Errors,
    };
    let (environment_file, environment_parsed) = read::environment_file(&stack, sources, d);
    let (written_profile, environment_choices) = match environment_file {
        Some(file) => (file.profile, Some(file.choices)),
        None => (None, None),
    };
    // The profile this command names replaces the one remembered.
    let asked_profile = read::command_lines(&options.asked)
        .into_iter()
        .rev()
        .find_map(|(asked, origin)| Some((asked.profile.as_deref()?, origin)));
    let profile = choose_profile(&stack, asked_profile, written_profile.as_ref(), sources, d)
        .map(|(profile, _)| profile);
    let production = profile.and_then(|profile| profile.production);
    let home = options.home.as_deref();
    let (places, places_parsed) = read::places(
        &stack,
        profile,
        home,
        environment_choices,
        &options.asked,
        sources,
        d,
    );
    let every_file_parsed = every_file_parsed && environment_parsed && places_parsed;
    let layered = choices::layer(places.in_order());
    // With a file that did not parse, the set of units is not known, and
    // every reference to a unit in it would be reported as missing; nor is
    // it known whether a place gives `up` or a required setting.
    // A profile that is named and missing, or that must be named and is
    // not, has been reported, and what it would have said is not known.
    let profile_missing = profile.is_none()
        && (asked_profile.is_some()
            || written_profile.is_some()
            || stack.default_profile.is_some()
            || stack.profiles.len() > 1);
    let action = match &options.action {
        Some(run) if every_file_parsed => action::asked(&stack, run, sources, d),
        _ => None,
    };
    if every_file_parsed {
        read::cross_references::check(&stack, &places.outside_the_stack(), d);
        if !profile_missing {
            choices::check_up_given(&stack, &layered, d);
        }
        if let Some(profile) = profile {
            choices::check_required(&stack, profile, &places.made(), d);
        }
    }
    if !d.is_empty() {
        return Outcome::Errors;
    }
    // A stack with no name has reported an error, so it does not get here.
    let Some(stack_name) = &stack.name else {
        return Outcome::Errors;
    };
    let shared_folders = options.shared_folders.as_deref();
    let Some(environment) = EnvironmentFacts::of(
        &stack,
        &stack_name.name,
        profile,
        &layered,
        shared_folders,
        d,
    ) else {
        return Outcome::Errors;
    };
    let (action, arguments) = action.unzip();
    let planning = Planning {
        stack: &stack,
        stack_name: &stack_name.name,
        layered: &layered,
        profile,
        production,
        root,
        environment,
        sought: [layered.up.as_slice(), action.as_slice()].concat(),
        arguments,
    };
    let Some(plan) = planning.plan(HOST_LISTEN_BEFORE_START, sources, d) else {
        return Outcome::Errors;
    };
    // `${host.listen}` is learnt from the environment's network so that
    // containers reach a host process. When nothing in the plan runs in a
    // container there is none to reach it, and it is the loopback address,
    // which needs no network. Where each unit runs is known only once the
    // plan is made, so a plan that would learn it is made again with it.
    let plan = match plan.uses_host_listen && !plan.runs_in_a_container() {
        true => planning.plan(Some(LOOPBACK), sources, d),
        false => Some(plan),
    };
    match plan {
        Some(plan) => Outcome::Plan(plan),
        None => Outcome::Errors,
    }
}

/// What the units that come up are planned from, once the definitions are
/// read and the choices applied.
struct Planning<'s> {
    stack: &'s Stack,
    stack_name: &'s str,
    layered: &'s Layered,
    profile: Option<&'s Profile>,
    /// Where the profile says it is a production one, when it does.
    production: Option<Loc>,
    root: &'s Path,
    environment: EnvironmentFacts,
    /// What `up` brings, and the action `tay run` runs.
    sought: Vec<NameAt>,
    arguments: Option<Arguments>,
}

impl Planning<'_> {
    /// The plan, with `${host.listen}` as `host_listen` when that is known
    /// before anything starts; `None` when it has errors, which go to `d`.
    fn plan(
        &self,
        host_listen: Option<&'static str>,
        sources: &mut Sources,
        d: &mut Diagnostics,
    ) -> Option<Plan> {
        let resolver = Resolver::new(
            self.stack,
            self.layered,
            self.environment.clone(),
            self.arguments.clone(),
            host_listen,
            sources,
        );
        let mut planner = Planner {
            resolver,
            root: self.root.to_path_buf(),
            production: self.production.is_some(),
            template_files: HashMap::new(),
        };
        let reached = planner.reach(&self.sought);
        if let (Some(profile), Some(marked_at)) = (self.profile, self.production) {
            planner.refuse_for_production(&profile.name, marked_at, &reached);
        }
        let up: Vec<String> = self
            .layered
            .up
            .iter()
            .map(|name| name.name.clone())
            .collect();
        let plan = planner.finish(reached, self.stack, self.stack_name, self.profile, up);
        planner.resolver.report_unset_settings();
        d.extend(planner.resolver.d);
        plan.filter(|_| d.is_empty())
    }
}

/// What chose the profile a plan is made with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChosenBy {
    /// `--profile`, now or remembered, as `tay set profile=` remembers it
    /// too.
    CommandLine,
    /// `profile:` in the environment's own file.
    EnvironmentFile,
    /// The stack's `default_profile`, or its only profile.
    StackDefault,
}

/// The profile a plan of `stack` is made with when `remembered` is the
/// profile the environment remembers being asked for, and what chose it,
/// as `tay status` names it; `None` when the stack has no profile to
/// choose, or the one chosen is none of its profiles, which planning
/// reports.
pub fn profile_in_force<'s>(
    stack: &'s Stack,
    remembered: Option<&str>,
) -> Option<(&'s Profile, ChosenBy)> {
    let mut sources = Sources::default();
    let mut d = Diagnostics::default();
    let (file, _) = read::environment_file(stack, &mut sources, &mut d);
    let written = file.and_then(|file| file.profile);
    let asked = remembered.map(|name| (name, Origin::Remembered));
    choose_profile(stack, asked, written.as_ref(), &mut sources, &mut d)
}

/// `--profile`, now or remembered, as `asked` says, or the profile this
/// environment's file names, or the stack's default profile, or its only
/// one, with which of these chose it.
fn choose_profile<'s>(
    stack: &'s Stack,
    asked: Option<(&str, Origin)>,
    written: Option<&NameAt>,
    sources: &mut Sources,
    d: &mut Diagnostics,
) -> Option<(&'s Profile, ChosenBy)> {
    let find = |name: &str| stack.profiles.iter().find(|p| p.name.name == name);
    let none_called = |name: &str| {
        format!(
            "there is no profile `{name}`; the profiles are {}",
            read::profile_list(&stack.profiles)
        )
    };
    // The environment's file names a profile that exists, whether or not
    // `--profile` overrules it.
    let written = written.map(|name| {
        let profile = find(&name.name);
        if profile.is_none() {
            d.error(name.loc, none_called(&name.name));
        }
        profile
    });
    let (profile, by) = match (asked, written, &stack.default_profile) {
        (Some((asked, origin)), _, _) => {
            let profile = find(asked);
            if profile.is_none() {
                let forgotten_by = format!("`tay unset {}` forgets it", Asked::PROFILE);
                let loc = origin.place(&format!("--profile {asked}"), &forgotten_by, sources);
                d.error(loc, none_called(asked));
            }
            (profile, ChosenBy::CommandLine)
        }
        (None, Some(written), _) => (written, ChosenBy::EnvironmentFile),
        (None, None, Some(default)) => (find(&default.name), ChosenBy::StackDefault),
        (None, None, None) => match stack.profiles.as_slice() {
            [only] => (Some(only), ChosenBy::StackDefault),
            _ => (None, ChosenBy::StackDefault),
        },
    };
    profile.map(|profile| (profile, by))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plan of the fixture stack `name`, with no personal choices.
    fn planned(name: &str, sets: &[&str]) -> Plan {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let root = fixtures.join(name).canonicalize().unwrap();
        let options = Options {
            asked: Asking {
                adds: Asked {
                    sets: sets.iter().map(|set| set.to_string()).collect(),
                    ..Asked::default()
                },
                ..Asking::default()
            },
            home: Some(fixtures.join("homes/none")),
            shared_folders: Some(fixtures.join("homes/none/.local/share/tay")),
            action: None,
        };
        let mut sources = Sources::default();
        let mut d = Diagnostics::default();
        match plan(&root, &options, &mut sources, &mut d) {
            Outcome::Plan(plan) => plan,
            _ => panic!("{name} plans: {:?}", d.into_sorted()),
        }
    }

    /// A unit's settings in the plan, as `Debug` writes them.
    fn settings(plan: &Plan, unit: &str) -> String {
        let unit = plan.units.iter().find(|u| u.name == unit).unwrap();
        let mechanism = unit.mechanism.as_ref().expect("the unit is not a group");
        format!("{:?}", mechanism.settings)
    }

    #[test]
    fn the_plan_holds_each_units_settings_with_every_value_resolved_where_it_runs() {
        let plan = planned("shop", &[]);
        let web = settings(&plan, "web");
        // The command's words, the port a placeholder that names its unit.
        assert!(
            web.contains(
                r#"command: [Text([Text("serve")]), Text([Text("--port")]), Placeholder(Port { unit: "web-port", name: "number" })]"#
            ),
            "{web}"
        );
        // The API's address as the host, where `web` runs, sees it: where
        // `${host.listen}` names, known before start on a Mac and learnt as
        // the environment comes up elsewhere; its port is left out if it is
        // filled with 80.
        let host = if cfg!(target_os = "macos") {
            r#"Text("http://127.0.0.1")"#
        } else {
            r#"Text("http://"), Placeholder(HostListen)"#
        };
        assert!(
            web.contains(&format!(
                r#"("API_URL", Text([{host}, UrlPort {{ scheme: Text([Text("http")]), port: Placeholder(Port {{ unit: "api-port", name: "number" }}) }}]))"#
            )),
            "{web}"
        );
        // A secret in `secret_env` names the secret that fills it.
        let database = settings(&plan, "database");
        assert!(
            database.contains(
                r#"secret_env: [("DB_PASSWORD", Placeholder(Generated { unit: "db-password", name: "value", secret: true }))]"#
            ),
            "{database}"
        );
    }

    #[test]
    fn a_ways_settings_are_the_units_joined_with_its_own_entry_by_entry() {
        // The template's unit-wide `env` is a mapping parameter, joined with
        // the way's own `env`; `command` is a list parameter.
        let service = settings(&planned("template-ok", &[]), "service");
        assert!(
            service.contains(r#"command: [Text([Text("serve")]), Text([Text("--fast")])]"#),
            "{service}"
        );
        assert!(
            service.contains(
                r#"env: [("MODE", Text([Text("fast")])), ("LEVEL", Text([Text("debug")]))]"#
            ),
            "{service}"
        );
    }

    #[test]
    fn a_secret_settings_value_is_kept_in_the_plan_under_its_name() {
        let database = settings(&planned("secrets-allowed", &[]), "database");
        assert!(
            database.contains(
                r#"("LICENCE", Placeholder(SecretSetting { name: "licence_key", value: Kept(..) }))"#
            ),
            "{database}"
        );
    }
}
