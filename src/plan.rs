//! The planner: from a stack's definitions and the environment's choices, the
//! units that come up, every value known before start, and the order units
//! could start in. It works from what modules say about units and never
//! asks which module a unit uses.

mod choices;
mod model;
mod order;
mod production;
mod reach;
mod resolve;

use std::path::{Path, PathBuf};

use crate::definition::read;
use crate::definition::{Choices, Profile, Stack};
use crate::diagnostic::source::{Loc, Sources};
use crate::diagnostic::{Diagnostic, Diagnostics};

pub use model::{
    Because, GivenLine, Mechanism, OpenedBy, Plan, PlannedUnit, PublishedPort, Readiness, Running,
    Seen, Shown,
};
pub use resolve::value::Exact;

use resolve::{Environment, Resolver};

/// What `tay plan` is asked for.
pub struct Options {
    pub profile: Option<String>,
    /// Each `--set KEY=VALUE`, in the order given.
    pub sets: Vec<String>,
    /// The home folder, where the personal choices file is.
    pub home: Option<PathBuf>,
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
    sources: &'s mut Sources,
    root: PathBuf,
    production: bool,
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
    let profile = choose_profile(&stack, options.profile.as_deref(), sources, d);
    let production = profile.and_then(|profile| profile.production);
    // A production stack must not change with whoever runs the command.
    let personal = match (&options.home, &stack.name, production) {
        (Some(home), Some(name), None) => {
            let path = home.join(".config/tay").join(format!("{}.yaml", name.name));
            read::optional_choices(&path, root, sources, d)
        }
        _ => None,
    };
    let sets: Vec<Choices> = options
        .sets
        .iter()
        .filter_map(|set| read::assignment(set, sources, d))
        .collect();
    let profile_choices = profile.map(|profile| &profile.choices);
    let places: Vec<&Choices> = [&stack.defaults]
        .into_iter()
        .chain(profile_choices)
        .chain(personal.as_ref())
        .chain(stack.environment_choices.as_ref())
        .chain(&sets)
        .collect();
    let layered = choices::layer(places.iter().copied());
    // With a file that did not parse, the set of units is not known, and
    // every reference to a unit in it would be reported as missing; nor is
    // it known whether a place gives `up` or a required setting.
    // A profile that is named and missing has been reported, and what it
    // would have said is not known.
    let profile_missing =
        profile.is_none() && (options.profile.is_some() || stack.default_profile.is_some());
    if every_file_parsed {
        read::references::check(&stack, &places, d);
        if !profile_missing {
            choices::check_up_given(&stack, &layered, d);
        }
        if let Some(profile) = profile {
            let counting: Vec<&Choices> = profile_choices
                .into_iter()
                .chain(stack.environment_choices.as_ref())
                .chain(&sets)
                .collect();
            choices::check_required(&stack, profile, &counting, d);
        }
    }
    if !d.is_empty() {
        return Outcome::Errors;
    }
    // A stack with no name has reported an error, so it does not get here.
    let Some(stack_name) = &stack.name else {
        return Outcome::Errors;
    };
    let home = options.home.as_deref();
    let Some(environment) = Environment::of(&stack, &stack_name.name, profile, &layered, home, d)
    else {
        return Outcome::Errors;
    };
    let folders = sources.folders();
    let mut planner = Planner {
        resolver: Resolver::new(&stack, &layered, environment, folders),
        sources,
        root: root.to_path_buf(),
        production: production.is_some(),
    };
    let reached = planner.reach(&layered.up);
    if let (Some(profile), Some(marked_at)) = (profile, production) {
        planner.refuse_for_production(&profile.name, marked_at, &reached.chosen);
    }
    let plan = planner.finish(reached, &stack, &stack_name.name, profile);
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
