//! The facts about this environment that references can use.

use std::path::Path;

use crate::definition::read::ENVIRONMENT_FOLDER;
use crate::definition::{Profile, Stack};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::plan::choices::Layered;
use crate::plan::resolve::resolved::Resolved;
use crate::syntax::expr::{EnvField, Reference};
use crate::value::{Hole, Value};

use super::{Res, Resolver};

#[derive(Debug)]
pub struct Environment {
    /// The name of the folder the stack's root is: in development the
    /// folder is the environment.
    pub name: String,
    pub label: String,
    pub root: String,
    pub state: String,
    pub logs: String,
    /// `None` when the home folder is not known.
    pub shared: Option<String>,
    pub profile: Option<String>,
}

impl Environment {
    /// The environment of `stack`'s folder under `profile` and the choices
    /// in force. Its paths are values, so the folder's path must be UTF-8.
    pub fn of(
        stack: &Stack,
        stack_name: &str,
        profile: Option<&Profile>,
        layered: &Layered,
        home: Option<&Path>,
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
        let label = match &layered.label {
            Some(chosen) => chosen.clone(),
            None => folder_label(&name, stack_name),
        };
        let own = |folder: &str| {
            Path::new(root)
                .join(ENVIRONMENT_FOLDER)
                .join(folder)
                .display()
                .to_string()
        };
        Some(Environment {
            label,
            name,
            root: root.to_string(),
            state: own("state"),
            logs: own("logs"),
            shared: home.and_then(|home| {
                home.join(".local/share/tay")
                    .join(stack_name)
                    .to_str()
                    .map(str::to_string)
            }),
            profile: profile.map(|profile| profile.name.name.clone()),
        })
    }
}

/// The label worked out from the folder's name: the name with the stack's
/// name and a hyphen taken off the front, cut down to what a hostname label
/// allows (lower-case letters, digits and single hyphens, at most 63
/// characters). Empty when the folder is named after the stack.
fn folder_label(folder: &str, stack: &str) -> String {
    if folder == stack {
        return String::new();
    }
    let rest = folder
        .strip_prefix(stack)
        .and_then(|rest| rest.strip_prefix('-'))
        .unwrap_or(folder);
    let mut label = String::new();
    for c in rest.to_lowercase().chars() {
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
        let affixed = |affix: fn(&str) -> String| match environment.label.as_str() {
            "" => Resolved::text(""),
            label => Resolved::text(affix(label)),
        };
        Ok(match field {
            EnvField::Name => Resolved::text(&environment.name),
            EnvField::Label => Resolved::text(&environment.label),
            EnvField::LabelSuffix => affixed(|label| format!("-{label}")),
            EnvField::LabelPrefix => affixed(|label| format!("{label}-")),
            EnvField::Root => Resolved::text(&environment.root),
            EnvField::State => Resolved::text(&environment.state),
            EnvField::Logs => Resolved::text(&environment.logs),
            EnvField::Shared => match &environment.shared {
                Some(shared) => Resolved::text(shared),
                None => {
                    return self.fail(Diagnostic::new(
                        reference.loc,
                        format!(
                            "`{}`: the home folder is not known, and the shared folder is in it",
                            reference.written()
                        ),
                    ));
                }
            },
            EnvField::Id => Resolved::known(Value::Hole(Hole::EnvironmentId)),
            EnvField::Network => Resolved::known(Value::Hole(Hole::EnvironmentNetwork)),
            EnvField::Profile => match &environment.profile {
                Some(profile) => Resolved::text(profile),
                None => {
                    return self.fail(Diagnostic::new(
                        reference.loc,
                        format!("`{}`: this stack has no profiles", reference.written()),
                    ));
                }
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_is_the_folder_name_after_the_stack_name_cut_down_to_a_hostname_label() {
        assert_eq!(folder_label("example-fix-login", "example"), "fix-login");
        assert_eq!(folder_label("example", "example"), "");
        assert_eq!(folder_label("Feature_Login", "example"), "feature-login");
        assert_eq!(folder_label("example-Fix..It--", "example"), "fix-it");
        assert_eq!(folder_label("examples", "example"), "examples");
        assert_eq!(folder_label("___", "example"), "");
        assert_eq!(folder_label(&"x".repeat(70), "example").len(), 63);
    }
}
