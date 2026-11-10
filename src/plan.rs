//! The planner: from a stack's definitions and the environment's choices, the
//! units that come up, every value known before start, and the order units
//! could start in. It works from what modules say about units and never
//! asks which module a unit uses.

mod choices;
mod model;
mod order;
mod production;
mod reach;
mod references;
mod resolve;
mod unit_plan;
mod usage;
mod value;

use std::path::{Path, PathBuf};

use crate::definition::{Choices, Profile, Stack};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::read::{self, ENVIRONMENT_FOLDER};
use crate::source::{Loc, Sources};

pub use model::{
    After, Because, GivenLine, Need, Plan, PlannedUnit, Readiness, RestartPolicy, Running, Seen,
    Shown, Way,
};
pub use value::Exact;

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
    let personal = match (&options.home, production) {
        (Some(home), None) if !stack.name.name.is_empty() => {
            let path = home
                .join(".config/tay")
                .join(format!("{}.yaml", stack.name.name));
            read::optional_choices(&path, root, sources, d)
        }
        _ => None,
    };
    let sets: Vec<Choices> = options
        .sets
        .iter()
        .filter_map(|set| read::assignment(set, sources, d))
        .collect();
    let places: Vec<&Choices> = profile
        .map(|profile| &profile.choices)
        .into_iter()
        .chain(personal.as_ref())
        .chain(stack.environment_choices.as_ref())
        .chain(&sets)
        .collect();
    // With a file that did not parse, the set of units is not known, and
    // every reference to a unit in it would be reported as missing.
    if every_file_parsed {
        references::check(&stack, &places, d);
    }
    if !d.is_empty() {
        return Outcome::Errors;
    }
    let layered = choices::layer(places);
    let Some(environment) = environment(&stack, profile, d) else {
        return Outcome::Errors;
    };
    let mut planner = Planner {
        resolver: Resolver::new(&stack, &layered, environment),
        sources,
        root: root.to_path_buf(),
        production: production.is_some(),
    };
    let reached = planner.reach(&layered.up);
    if let (Some(profile), Some(marked_at)) = (profile, production) {
        planner.refuse_for_production(&profile.name, marked_at, &reached.chosen);
    }
    planner.report_unset_settings();
    let plan = planner.finish(reached, &stack, profile);
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

fn environment(
    stack: &Stack,
    profile: Option<&Profile>,
    d: &mut Diagnostics,
) -> Option<Environment> {
    let Some(root) = stack.root.to_str() else {
        d.error(
            stack.loc,
            "the path of the stack's folder is not valid UTF-8, so it cannot be used as a value",
        );
        return None;
    };
    let name = stack
        .root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    Some(Environment {
        label: hostname_label(&name),
        name,
        root: root.to_string(),
        state: stack
            .root
            .join(ENVIRONMENT_FOLDER)
            .join("state")
            .to_str()
            .expect("the root is UTF-8, and so is this path under it")
            .to_string(),
        profile: profile.map(|profile| profile.name.name.clone()),
    })
}

/// A name cut down to what a hostname label allows: lower-case letters,
/// digits and single hyphens, at most 63 characters.
fn hostname_label(name: &str) -> String {
    let mut label = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            label.push(c);
        } else if !label.ends_with('-') {
            label.push('-');
        }
    }
    let label = label.trim_matches('-');
    label[..label.len().min(63)]
        .trim_end_matches('-')
        .to_string()
}

impl Planner<'_> {
    fn report_unset_settings(&mut self) {
        let stack = self.resolver.stack;
        for (name, uses) in std::mem::take(&mut self.resolver.unset) {
            let Some(decl) = stack.setting(&name) else {
                continue;
            };
            let mut uses = uses;
            uses.sort();
            uses.dedup();
            let mut error = Diagnostic::new(
                decl.name.loc,
                format!("setting `{name}` has no value and no default, and the plan uses it"),
            );
            for loc in uses {
                error = error.with(loc, "it is used here");
            }
            self.resolver.d.push(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_name_is_cut_down_to_a_hostname_label() {
        assert_eq!(hostname_label("Feature_Login"), "feature-login");
        assert_eq!(hostname_label("--a..b--"), "a-b");
        assert_eq!(hostname_label("___"), "");
        assert_eq!(hostname_label(&"x".repeat(70)).len(), 63);
    }
}
