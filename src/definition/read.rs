//! Reading a stack's definitions from its folder: `tay.yaml`, the units, the
//! templates and the profiles, and the environment's own choices.

mod choices;
mod files;
pub mod references;
mod settings;
mod unit;
mod version;

use std::collections::HashMap;
use std::path::Path;

use crate::definition::{Choices, Profile, Stack, Unit};
use crate::diagnostic::source::{Loc, Sources};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt, RESERVED_UNIT_NAMES};

use files::Reader;

pub use choices::{assignment, choices_file};
pub use files::discover::{STACK_FILE, find_root};
pub use version::THIS_TAY;

/// The folder inside a stack's root that holds its environment's own choices and state.
pub const ENVIRONMENT_FOLDER: &str = ".tay";

pub enum Outcome {
    /// The definitions, as far as they could be read. Errors are in the
    /// diagnostics; `every_file_parsed` is false when some file was not valid
    /// YAML, so the set of units itself is not known.
    Read {
        stack: Box<Stack>,
        every_file_parsed: bool,
    },
    /// This tay may not read these definitions; each line says why.
    TooOld(Vec<Diagnostic>),
    /// `tay.yaml` itself could not be read.
    Unreadable,
}

const STACK_FIELDS: &[&str] = &[
    "format",
    "requires",
    "stack",
    "default_profile",
    "units_dir",
    "templates_dir",
    "profiles_dir",
    "settings",
    "up",
    "run",
    "units",
];

pub fn read(root: &Path, sources: &mut Sources, d: &mut Diagnostics) -> Outcome {
    let mut reader = Reader::new(root, sources, d);
    let Some(node) = reader.parse(&root.join(STACK_FILE)) else {
        return Outcome::Unreadable;
    };
    let Some(file) = Fields::of(&node, "tay.yaml", STACK_FIELDS, reader.d) else {
        return Outcome::Unreadable;
    };
    if let Err(reasons) = version::gate(&file, reader.d) {
        return Outcome::TooOld(reasons);
    }
    let stack_name = file
        .require("stack", reader.d)
        .and_then(|node| fields::name(node, "the stack's name", reader.d));
    let default_profile = file
        .get("default_profile")
        .and_then(|node| fields::name(node, "`default_profile`", reader.d));
    let units_dir = reader.folder(&file, "units_dir", "stack/units");
    let templates_dir = reader.folder(&file, "templates_dir", "stack/templates");
    let profiles_dir = reader.folder(&file, "profiles_dir", "stack/profiles");
    let settings = file
        .get("settings")
        .map(|node| settings::declarations(node, reader.d))
        .unwrap_or_default();
    let defaults = choices::stack_defaults(&file, reader.d);
    let templates = reader.templates(templates_dir);
    let mut units = Vec::new();
    if let Some(inline) = file.get("units") {
        for (name, node) in fields::keyed(inline, "a unit's name", names::name_problem, reader.d) {
            units.push(unit::read(name, node, &templates, reader.d));
        }
    }
    units.extend(reader.unit_files(units_dir, &templates));
    let units = unique_units(units, reader.d);
    unique_claims(&units, reader.d);
    let profiles = reader.profiles(profiles_dir);
    check_default_profile(node.loc, default_profile.as_ref(), &profiles, reader.d);
    let environment_choices = reader.environment_choices();
    let stack = Stack {
        root: root.to_path_buf(),
        name: stack_name,
        loc: node.loc,
        default_profile,
        settings,
        units,
        profiles,
        defaults,
        environment_choices,
    };
    Outcome::Read {
        stack: Box::new(stack),
        every_file_parsed: reader.every_file_parsed,
    }
}

/// Reads this person's own choices for the stack `stack`,
/// `~/.config/tay/<stack>.yaml`, if the file exists.
pub fn personal_choices(
    home: &Path,
    stack: &str,
    root: &Path,
    sources: &mut Sources,
    d: &mut Diagnostics,
) -> Option<Choices> {
    let path = home.join(".config/tay").join(format!("{stack}.yaml"));
    if !path.is_file() {
        return None;
    }
    let mut reader = Reader::new(root, sources, d);
    let node = reader.parse(&path)?;
    choices_file(&node, reader.d)
}

/// Unit names are unique across the stack, and none is a name references use.
fn unique_units(units: Vec<Unit>, d: &mut Diagnostics) -> Vec<Unit> {
    let mut kept: Vec<Unit> = Vec::new();
    for unit in units {
        if RESERVED_UNIT_NAMES.contains(&unit.name.name.as_str()) {
            d.error(
                unit.name.loc,
                format!(
                    "`{}` cannot be a unit's name: references use it",
                    unit.name.name
                ),
            );
            continue;
        }
        match kept.iter().find(|k| k.name.name == unit.name.name) {
            Some(first) => d.push(
                Diagnostic::new(
                    unit.name.loc,
                    format!("there is already a unit called `{}`", unit.name.name),
                )
                .with(first.name.loc, "the first one is defined here"),
            ),
            None => kept.push(unit),
        }
    }
    kept
}

/// No two units make the same thing, such as a file at one place in the
/// state folder. The ways of one unit may, since only one of them runs.
fn unique_claims(units: &[Unit], d: &mut Diagnostics) {
    let mut first: HashMap<&str, (&str, Loc)> = HashMap::new();
    for unit in units {
        let claims = unit
            .ways
            .iter()
            .filter_map(|way| way.module.as_ref())
            .flat_map(|module| &module.shape.claims);
        for claim in claims {
            match first.get(claim.what.as_str()) {
                Some((owner, _)) if *owner == unit.name.name => {}
                Some((owner, at)) => d.push(
                    Diagnostic::new(
                        claim.loc,
                        format!(
                            "unit `{}` makes {}, as unit `{owner}` does",
                            unit.name.name, claim.what
                        ),
                    )
                    .with(*at, format!("`{owner}` makes it here")),
                ),
                None => {
                    first.insert(&claim.what, (&unit.name.name, claim.loc));
                }
            }
        }
    }
}

fn check_default_profile(
    at: Loc,
    default: Option<&NameAt>,
    profiles: &[Profile],
    d: &mut Diagnostics,
) {
    match default {
        Some(default) if !profiles.iter().any(|p| p.name.name == default.name) => d.error(
            default.loc,
            format!(
                "there is no profile `{}`; the profiles are {}",
                default.name,
                profile_list(profiles)
            ),
        ),
        None if profiles.len() > 1 => d.error(
            at,
            format!(
                "there is more than one profile ({}), so `default_profile` is required",
                profile_list(profiles)
            ),
        ),
        _ => {}
    }
}

pub fn profile_list(profiles: &[Profile]) -> String {
    if profiles.is_empty() {
        return "none".to_string();
    }
    profiles
        .iter()
        .map(|p| format!("`{}`", p.name.name))
        .collect::<Vec<_>>()
        .join(", ")
}
