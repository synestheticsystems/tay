//! The planner: from a stack's definitions and the environment's choices, the
//! units that come up, every value known before start, and the order units
//! could start in. It works from what modules say about units and never
//! asks which module a unit uses.

mod assemble;
mod choices;
mod model;
mod order;
mod production;
mod reach;
mod resolve;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::definition::read;
use crate::definition::{Profile, Stack};
use crate::diagnostic::source::{Loc, Sources};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::disk::Asked;
use crate::syntax::expr::Expr;

pub use model::{
    Because, Given, OpenedBy, Plan, PlannedMechanism, PlannedUnit, PublishedPort, Readiness, Seen,
    Unreachable,
};

use resolve::{EnvironmentFacts, Resolver};

/// What `tay plan` is asked for, or `tay up`, which plans as it does.
pub struct Options {
    /// The profile, each `--set` in the order it applies, and the units
    /// named in place of `up`: what the environment remembers being asked
    /// for, with what this command adds.
    pub asked: Asked,
    /// The home folder, where this person's own file of choices is.
    pub home: Option<PathBuf>,
    /// The folder that holds each stack's shared folder, when it is known.
    pub shared_folders: Option<PathBuf>,
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
    let profile = choose_profile(&stack, options.asked.profile.as_deref(), sources, d);
    let production = profile.and_then(|profile| profile.production);
    let home = options.home.as_deref();
    let (places, places_parsed) = read::places(&stack, profile, home, &options.asked, sources, d);
    let every_file_parsed = every_file_parsed && places_parsed;
    let layered = choices::layer(places.in_order());
    // With a file that did not parse, the set of units is not known, and
    // every reference to a unit in it would be reported as missing; nor is
    // it known whether a place gives `up` or a required setting.
    // A profile that is named and missing, or that must be named and is
    // not, has been reported, and what it would have said is not known.
    let profile_missing = profile.is_none()
        && (options.asked.profile.is_some()
            || stack.default_profile.is_some()
            || stack.profiles.len() > 1);
    if every_file_parsed {
        read::cross_references::check(&stack, &places.in_order(), d);
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
    let mut planner = Planner {
        resolver: Resolver::new(&stack, &layered, environment, sources),
        root: root.to_path_buf(),
        production: production.is_some(),
        template_files: HashMap::new(),
    };
    let reached = planner.reach(&layered.up);
    if let (Some(profile), Some(marked_at)) = (profile, production) {
        planner.refuse_for_production(&profile.name, marked_at, &reached);
    }
    let up: Vec<String> = layered.up.iter().map(|name| name.name.clone()).collect();
    let plan = planner.finish(reached, &stack, &stack_name.name, profile, up);
    planner.resolver.report_unset_settings();
    d.extend(planner.resolver.d);
    match plan {
        Some(plan) if d.is_empty() => Outcome::Plan(plan),
        _ => Outcome::Errors,
    }
}

/// `--profile`, or the stack's default profile, or its only one.
fn choose_profile<'s>(
    stack: &'s Stack,
    asked: Option<&str>,
    sources: &mut Sources,
    d: &mut Diagnostics,
) -> Option<&'s Profile> {
    let find = |name: &str| stack.profiles.iter().find(|p| p.name.name == name);
    match (asked, &stack.default_profile) {
        (Some(asked), _) => {
            let profile = find(asked);
            if profile.is_none() {
                let loc = Loc::new(sources.add_argument(&format!("--profile {asked}")), 1, 1);
                d.error(
                    loc,
                    format!(
                        "there is no profile `{asked}`; the profiles are {}",
                        read::profile_list(&stack.profiles)
                    ),
                );
            }
            profile
        }
        (None, Some(default)) => find(&default.name),
        (None, None) => match stack.profiles.as_slice() {
            [only] => Some(only),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plan of the fixture stack `name`, with no personal choices.
    fn planned(name: &str, sets: &[&str]) -> Plan {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let root = fixtures.join(name).canonicalize().unwrap();
        let options = Options {
            asked: Asked {
                sets: sets.iter().map(|set| set.to_string()).collect(),
                ..Asked::default()
            },
            home: Some(fixtures.join("homes/none")),
            shared_folders: Some(fixtures.join("homes/none/.local/share/tay")),
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
        // The API's address as the host, where `web` runs, sees it.
        assert!(
            web.contains(
                r#"("API_URL", Text([Text("http://127.0.0.1:"), Placeholder(Port { unit: "api-port", name: "number" })]))"#
            ),
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
