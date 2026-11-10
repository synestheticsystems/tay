//! Reading a stack's definitions from its folder: `tay.yaml`, the units, the
//! templates and the profiles, and the environment's own choices.

mod choices;
mod discover;
mod settings;
mod template;
mod unit;
mod unit_parts;
mod version;

use std::fs;
use std::path::{Path, PathBuf};

use crate::definition::{Choices, Profile, Stack, Unit};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::fields::{self, Fields};
use crate::names::{self, NameAt, RESERVED_UNIT_NAMES};
use crate::source::{Loc, Sources};
use crate::yaml::{self, Node};

pub use choices::{assignment, choices_file};
pub use discover::{STACK_FILE, find_root};
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
    "units",
];

pub fn read(root: &Path, sources: &mut Sources, d: &mut Diagnostics) -> Outcome {
    let mut reader = Reader {
        root,
        sources,
        d,
        every_file_parsed: true,
    };
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
    let templates = reader.templates(templates_dir);
    let mut units = Vec::new();
    if let Some(inline) = file.get("units") {
        for (name, node) in fields::keyed(inline, "a unit's name", names::name_problem, reader.d) {
            units.push(unit::read(name, node, &templates, reader.d));
        }
    }
    units.extend(reader.unit_files(units_dir, &templates));
    let units = unique_units(units, reader.d);
    let profiles = reader.profiles(profiles_dir);
    check_default_profile(node.loc, default_profile.as_ref(), &profiles, reader.d);
    let environment_choices = reader.environment_choices();
    let stack = Stack {
        root: root.to_path_buf(),
        name: stack_name.unwrap_or(NameAt {
            name: String::new(),
            loc: node.loc,
        }),
        loc: node.loc,
        default_profile,
        settings,
        units,
        profiles,
        environment_choices,
    };
    Outcome::Read {
        stack: Box::new(stack),
        every_file_parsed: reader.every_file_parsed,
    }
}

/// Reads a choices file outside the stack (the personal one), if it exists.
pub fn optional_choices(
    path: &Path,
    root: &Path,
    sources: &mut Sources,
    d: &mut Diagnostics,
) -> Option<Choices> {
    if !path.is_file() {
        return None;
    }
    let mut reader = Reader {
        root,
        sources,
        d,
        every_file_parsed: true,
    };
    let node = reader.parse(path)?;
    choices_file(&node, reader.d)
}

struct Reader<'a> {
    root: &'a Path,
    sources: &'a mut Sources,
    d: &'a mut Diagnostics,
    every_file_parsed: bool,
}

/// A definitions folder, and where `tay.yaml` names it when it does.
struct Folder {
    path: PathBuf,
    named_at: Option<Loc>,
}

impl Reader<'_> {
    fn parse(&mut self, path: &Path) -> Option<Node> {
        let source = self.sources.add_file(path, self.root);
        match fs::read_to_string(path) {
            Ok(text) => {
                let node = yaml::parse(&text, source, self.d);
                self.every_file_parsed &= node.is_some();
                node
            }
            Err(error) => {
                self.d.error(
                    Loc::new(source, 1, 1),
                    format!("this file cannot be read: {error}"),
                );
                self.every_file_parsed = false;
                None
            }
        }
    }

    fn folder(&mut self, file: &Fields<'_>, key: &str, default: &str) -> Folder {
        match file.get(key) {
            Some(node) => Folder {
                path: self
                    .root
                    .join(fields::text(node, &format!("`{key}`"), self.d).unwrap_or(default)),
                named_at: Some(node.loc),
            },
            None => Folder {
                path: self.root.join(default),
                named_at: None,
            },
        }
    }

    /// The entries of a definitions folder. A folder `tay.yaml` names must
    /// exist; one it leaves at the default may be absent, as in a small stack
    /// that keeps everything in `tay.yaml`.
    fn entries(&mut self, folder: &Folder) -> Vec<discover::Entry> {
        match discover::entries(&folder.path) {
            Ok(Some(entries)) => entries,
            Ok(None) => {
                if let Some(loc) = folder.named_at {
                    self.d.error(
                        loc,
                        format!("the folder `{}` does not exist", folder.path.display()),
                    );
                }
                Vec::new()
            }
            Err(error) => {
                let loc = folder.named_at.unwrap_or_else(|| {
                    Loc::new(self.sources.add_file(&folder.path, self.root), 1, 1)
                });
                self.d
                    .error(loc, format!("this folder cannot be read: {error}"));
                self.every_file_parsed = false;
                Vec::new()
            }
        }
    }

    fn not_a_definition(&mut self, path: &Path, holds: &str) {
        let loc = Loc::new(self.sources.add_file(path, self.root), 1, 1);
        let hint = if path.extension().is_some_and(|ext| ext == "yml") {
            " (tay reads `.yaml`, not `.yml`)"
        } else {
            ""
        };
        self.d.error(
            loc,
            format!("this is not a definition: this folder holds {holds}{hint}"),
        );
    }

    /// The name a file or folder gives what it defines.
    fn file_name(&mut self, path: &Path, node: &Node, what: &str) -> Option<NameAt> {
        let name = discover::stem(path);
        match names::name_problem(&name) {
            None => Some(NameAt {
                name,
                loc: node.loc,
            }),
            Some(problem) => {
                self.d.error(
                    node.loc,
                    format!("the file's name is {what}'s name: {problem}"),
                );
                None
            }
        }
    }

    fn templates(&mut self, folder: Folder) -> Vec<template::Template> {
        let mut templates = Vec::new();
        for entry in self.entries(&folder) {
            match entry {
                discover::Entry::File(path) => {
                    let Some(node) = self.parse(&path) else {
                        continue;
                    };
                    let Some(name) = self.file_name(&path, &node, "the template") else {
                        continue;
                    };
                    templates.extend(template::read(name, &node, self.d));
                }
                discover::Entry::Folder(path) | discover::Entry::Other(path) => {
                    self.not_a_definition(&path, "one `<name>.yaml` file per template");
                }
            }
        }
        templates
    }

    fn unit_files(&mut self, folder: Folder, templates: &[template::Template]) -> Vec<Unit> {
        let mut units = Vec::new();
        for entry in self.entries(&folder) {
            match entry {
                discover::Entry::Folder(path) => {
                    let file = path.join("unit.yaml");
                    if !file.is_file() {
                        self.not_a_definition(&path, "`<name>/unit.yaml` and `<anything>.yaml`, and this folder has no `unit.yaml`");
                        continue;
                    }
                    let Some(node) = self.parse(&file) else {
                        continue;
                    };
                    let Some(name) = self.file_name(&path, &node, "the unit") else {
                        continue;
                    };
                    units.push(unit::read(name, &node, templates, self.d));
                }
                discover::Entry::File(path) => {
                    let Some(node) = self.parse(&path) else {
                        continue;
                    };
                    for (name, unit) in
                        fields::keyed(&node, "a unit's name", names::name_problem, self.d)
                    {
                        units.push(unit::read(name, unit, templates, self.d));
                    }
                }
                discover::Entry::Other(path) => {
                    self.not_a_definition(&path, "`<name>/unit.yaml` and `<anything>.yaml`");
                }
            }
        }
        units
    }

    fn profiles(&mut self, folder: Folder) -> Vec<Profile> {
        let mut profiles = Vec::new();
        for entry in self.entries(&folder) {
            match entry {
                discover::Entry::File(path) => {
                    let Some(node) = self.parse(&path) else {
                        continue;
                    };
                    let Some(name) = self.file_name(&path, &node, "the profile") else {
                        continue;
                    };
                    profiles.extend(choices::profile(name, &node, self.d));
                }
                discover::Entry::Folder(path) | discover::Entry::Other(path) => {
                    self.not_a_definition(&path, "one `<name>.yaml` file per profile");
                }
            }
        }
        profiles
    }

    fn environment_choices(&mut self) -> Option<Choices> {
        let path = self.root.join(ENVIRONMENT_FOLDER).join("choices.yaml");
        if !path.is_file() {
            return None;
        }
        let node = self.parse(&path)?;
        choices_file(&node, self.d)
    }
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
