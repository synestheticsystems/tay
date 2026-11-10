//! The facts about this environment that references can use.

use std::path::Path;

use crate::definition::{Profile, Stack};
use crate::diagnostic::Diagnostics;
use crate::expr::{EnvField, Reference, Type};
use crate::plan::value::{Hole, Resolved, Shape};
use crate::read::ENVIRONMENT_FOLDER;

use super::{Res, Resolver};

#[derive(Debug)]
pub struct Environment {
    /// The name of the stack's folder: in development the folder is the environment.
    pub name: String,
    pub label: String,
    pub root: String,
    pub state: String,
    pub profile: Option<String>,
}

impl Environment {
    /// The environment of `stack`'s folder under `profile`. Its paths are
    /// values, so the folder's path must be UTF-8.
    pub fn of(
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
            state: Path::new(root)
                .join(ENVIRONMENT_FOLDER)
                .join("state")
                .display()
                .to_string(),
            profile: profile.map(|profile| profile.name.name.clone()),
        })
    }
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

impl Resolver<'_> {
    pub(super) fn environment_field(&mut self, field: EnvField, reference: &Reference) -> Res {
        let environment = &self.environment;
        Ok(match field {
            EnvField::Name => Resolved::text(&environment.name),
            EnvField::Label => {
                if environment.label.is_empty() {
                    return self.fail(
                        reference.loc,
                        format!(
                            "`{}`: the environment's name `{}` leaves nothing a hostname may hold",
                            reference.written(),
                            environment.name
                        ),
                    );
                }
                Resolved::text(&environment.label)
            }
            EnvField::Root => Resolved::text(&environment.root),
            EnvField::State => Resolved::text(&environment.state),
            EnvField::Id => Resolved::known(Shape::Opaque {
                ty: Some(Type::Text),
                hole: Hole::EnvironmentId,
            }),
            EnvField::Profile => match &environment.profile {
                Some(profile) => Resolved::text(profile),
                None => {
                    return self.fail(
                        reference.loc,
                        format!("`{}`: this stack has no profiles", reference.written()),
                    );
                }
            },
        })
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
