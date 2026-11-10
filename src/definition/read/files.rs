//! Reading the definition files in a stack's folders, each file once, with
//! nothing in them skipped.

pub mod discover;

use std::fs;
use std::path::{Path, PathBuf};

use crate::definition::{Choices, Profile, Unit};
use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::{Loc, Sources};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::{self, Node};

use super::unit::{self, template};
use super::{ENVIRONMENT_FOLDER, choices, choices_file};

pub struct Reader<'a> {
    root: &'a Path,
    sources: &'a mut Sources,
    pub d: &'a mut Diagnostics,
    /// False once a file is not valid YAML or cannot be read.
    pub every_file_parsed: bool,
}

/// A definitions folder, and where `tay.yaml` names it when it does.
pub struct Folder {
    path: PathBuf,
    named_at: Option<Loc>,
}

impl<'a> Reader<'a> {
    pub fn new(root: &'a Path, sources: &'a mut Sources, d: &'a mut Diagnostics) -> Reader<'a> {
        Reader {
            root,
            sources,
            d,
            every_file_parsed: true,
        }
    }

    pub fn parse(&mut self, path: &Path) -> Option<Node> {
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

    /// The folder `key` names, or `default`. `None` when `key` is not text:
    /// that folder is then not read, and the set of units is not known.
    pub fn folder(&mut self, file: &Fields<'_>, key: &str, default: &str) -> Option<Folder> {
        let Some(node) = file.get(key) else {
            return Some(Folder {
                path: self.root.join(default),
                named_at: None,
            });
        };
        let Some(path) = fields::text(node, &format!("`{key}`"), self.d) else {
            self.every_file_parsed = false;
            return None;
        };
        Some(Folder {
            path: self.root.join(path),
            named_at: Some(node.loc),
        })
    }

    /// The entries of a definitions folder. A folder `tay.yaml` names must
    /// exist; one it leaves at the default may be absent, as in a small stack
    /// that keeps everything in `tay.yaml`.
    fn entries(&mut self, folder: Option<Folder>) -> Vec<discover::Entry> {
        let Some(folder) = folder else {
            return Vec::new();
        };
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

    pub fn templates(&mut self, folder: Option<Folder>) -> Vec<template::Template> {
        let mut templates = Vec::new();
        for entry in self.entries(folder) {
            match entry {
                discover::Entry::File(path) => {
                    let Some(node) = self.parse(&path) else {
                        continue;
                    };
                    let Some(name) = self.file_name(&path, &node, "the template") else {
                        continue;
                    };
                    templates.push(template::read(name, &node, self.d));
                }
                discover::Entry::Folder(path) | discover::Entry::Other(path) => {
                    self.not_a_definition(&path, "one `<name>.yaml` file per template");
                }
            }
        }
        templates
    }

    pub fn unit_files(
        &mut self,
        folder: Option<Folder>,
        templates: &[template::Template],
    ) -> Vec<Unit> {
        let mut units = Vec::new();
        for entry in self.entries(folder) {
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
                    let defined =
                        fields::keyed(&node, "a unit's name", names::name_problem, self.d);
                    if defined.is_empty() && node.as_map().is_some() {
                        self.d
                            .error(node.loc, "this file defines no unit: its mapping is empty");
                    }
                    for (name, unit) in defined {
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

    pub fn profiles(&mut self, folder: Option<Folder>) -> Vec<Profile> {
        let mut profiles = Vec::new();
        for entry in self.entries(folder) {
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

    pub fn environment_choices(&mut self) -> Option<Choices> {
        let path = self.root.join(ENVIRONMENT_FOLDER).join("choices.yaml");
        if !path.is_file() {
            return None;
        }
        let node = self.parse(&path)?;
        choices_file(&node, self.d)
    }
}
