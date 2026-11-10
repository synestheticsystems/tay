//! The rules names follow. A rule returns what is wrong with a name, if anything.

use crate::source::Loc;

/// A name and the place it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameAt {
    pub name: String,
    pub loc: Loc,
}

/// Names references use for themselves, so no unit may have them.
pub const RESERVED_UNIT_NAMES: [&str; 5] = ["settings", "environment", "host", "self", "params"];

/// The rule for unit, template, profile and alternative names: lower-case
/// letters, digits and hyphens, starting with a letter, at most 40 characters.
pub fn name_problem(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("a name cannot be empty".to_string());
    }
    if name.contains("${") {
        return Some(format!(
            "`{name}` is a reference, and a name must be written out"
        ));
    }
    if name.chars().count() > 40 {
        return Some(format!("`{name}` is longer than 40 characters"));
    }
    if !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        return Some(format!("`{name}` must start with a lower-case letter"));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-'))
    {
        return Some(format!(
            "`{name}` may hold only lower-case letters, digits and hyphens, not `{bad}`"
        ));
    }
    None
}

/// The rule for the names of values: what a unit gives, produces or builds, a
/// template's parameters. The format does not state one; references need
/// the name to be one dotted part, so it is a lower-case letter followed by
/// lower-case letters, digits, hyphens and underscores.
pub fn value_name_problem(name: &str) -> Option<String> {
    if name.contains("${") {
        return Some(format!(
            "`{name}` is a reference, and a name must be written out"
        ));
    }
    if !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        return Some(format!("`{name}` must start with a lower-case letter"));
    }
    if let Some(bad) = name.chars().find(|c| !is_value_char(*c)) {
        return Some(format!(
            "`{name}` may hold only lower-case letters, digits, hyphens and underscores, not `{bad}`"
        ));
    }
    None
}

fn is_value_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
}

/// The rule for setting names: dotted, each part lower-case letters, digits,
/// hyphens or underscores.
pub fn setting_name_problem(name: &str) -> Option<String> {
    if name.split('.').any(str::is_empty) {
        return Some(format!("`{name}` has an empty part between dots"));
    }
    if let Some(bad) = name.chars().find(|c| !(is_value_char(*c) || *c == '.')) {
        return Some(format!(
            "`{name}` may hold only lower-case letters, digits, hyphens, underscores and dots, not `{bad}`"
        ));
    }
    None
}

/// Environment variable names, as a shell can use them.
pub fn variable_name_problem(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let starts_well = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !starts_well || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Some(format!(
            "`{name}` is not an environment variable name: letters, digits and underscores, not starting with a digit"
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_names() {
        assert_eq!(name_problem("web-port"), None);
        assert_eq!(name_problem("a1"), None);
        assert!(name_problem("Web").is_some());
        assert!(name_problem("1web").is_some());
        assert!(name_problem("web_port").is_some());
        assert!(name_problem(&"a".repeat(41)).is_some());
        assert_eq!(name_problem(&"a".repeat(40)), None);
    }

    #[test]
    fn value_and_setting_names() {
        assert_eq!(value_name_problem("api_url"), None);
        assert!(value_name_problem("Api").is_some());
        assert_eq!(setting_name_problem("log.web"), None);
        assert_eq!(setting_name_problem("images.server_v2"), None);
        assert!(setting_name_problem("log..web").is_some());
        assert!(setting_name_problem("Log.web").is_some());
    }

    #[test]
    fn variable_names() {
        assert_eq!(variable_name_problem("DATABASE_URL"), None);
        assert_eq!(variable_name_problem("_x1"), None);
        assert!(variable_name_problem("1X").is_some());
        assert!(variable_name_problem("A-B").is_some());
    }
}
