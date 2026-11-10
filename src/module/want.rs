//! What a value must be where it is used, checked against what is known of it.

use crate::syntax::expr::{Expr, ExprKind};

use super::shape::Place;

/// What a value must be once resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Want {
    Text,
    Number,
    Bool,
    /// Text, a number or true or false: what becomes one word of a command
    /// line or one environment variable.
    Scalar,
    PortNumber,
    /// A whole number above 0: how many of something.
    Count,
    OneOf(&'static [&'static str]),
    /// A whole number followed by `s`, `m` or `h`.
    Duration,
    /// Permissions written as octal text, such as `"0600"`.
    FileMode,
    /// A file's name, with no folder in it.
    FileName,
    /// A path on the host. One that begins `./` is relative to the folder of
    /// the file it is written in, any other relative one to the stack's
    /// root, and the value used is the whole path.
    Path(PathUse),
}

/// What tay does with a path before start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathUse {
    /// Nothing: it is for whatever runs the unit.
    Named,
    /// A mount's source, which must exist when tay plans unless it lies in
    /// the state or shared folder, where tay makes it, or is built from a
    /// value produced by running.
    Mounted,
    /// A file whose text is read before start and has its references filled
    /// in; the text goes to the place given.
    Read(Place),
}

/// What is known of a value when it is checked against a [`Want`]: its type,
/// and its exact form when that is known.
#[derive(Clone, Debug, PartialEq)]
pub enum Form {
    Text(Option<String>),
    Number(Option<i64>),
    Bool,
    /// Produced by running: its type is not known before start.
    Unknown,
}

impl Form {
    /// The form of a value as written, or `None` when a reference in it
    /// decides it: what the reference brings is known only once it is
    /// resolved.
    pub fn of_written(expr: &Expr) -> Option<Form> {
        match &expr.kind {
            ExprKind::Number(n) => Some(Form::Number(Some(*n))),
            ExprKind::Bool(_) => Some(Form::Bool),
            ExprKind::Text(_) => expr.literal_text().map(|text| Form::Text(Some(text))),
            ExprKind::Ref(_) => None,
        }
    }

    fn describe(&self) -> &'static str {
        match self {
            Form::Text(_) => "text",
            Form::Number(_) => "a whole number",
            Form::Bool => "true or false",
            Form::Unknown => "produced by running",
        }
    }
}

impl Want {
    /// What is wrong with a value of this form here, if anything. A value
    /// produced by running passes every check that its type would decide,
    /// since its type is not known before start.
    pub fn problem(&self, form: &Form) -> Option<String> {
        let wrong = |wanted: &str| Some(format!("must be {wanted}; this is {}", form.describe()));
        match (self, form) {
            (Want::Path(PathUse::Read(_)), Form::Unknown) => Some(
                "must be known before start, because tay reads the file then; this is produced by running"
                    .to_string(),
            ),
            (_, Form::Unknown) | (Want::Scalar, _) => None,
            (Want::Path(PathUse::Named | PathUse::Mounted), Form::Text(_)) => None,
            (Want::Text, Form::Text(_)) | (Want::Bool, Form::Bool) => None,
            (Want::Number, Form::Number(_)) => None,
            (Want::Text, _) => wrong("text"),
            (Want::Number, _) => wrong("a whole number"),
            (Want::Bool, _) => wrong("true or false"),
            (Want::PortNumber, Form::Number(n)) => match n {
                Some(n) if !(1..=65535).contains(n) => {
                    Some(format!("must be a port number from 1 to 65535; this is {n}"))
                }
                _ => None,
            },
            (Want::PortNumber, _) => wrong("a port number"),
            (Want::Count, Form::Number(Some(n))) if *n < 1 => {
                Some(format!("must be a whole number above 0; this is {n}"))
            }
            (Want::Count, Form::Number(_)) => None,
            (Want::Count, _) => wrong("a whole number"),
            (_, Form::Number(_) | Form::Bool) => wrong("text"),
            (want, Form::Text(None)) => Some(format!(
                "must be written out in full ({}); this holds a value not known until start",
                want.expects()
            )),
            (want, Form::Text(Some(text))) => (!want.accepts(text))
                .then(|| format!("must be {}; this is `{text}`", want.expects())),
        }
    }

    fn accepts(&self, text: &str) -> bool {
        match self {
            Want::OneOf(choices) => choices.contains(&text),
            Want::Duration => is_duration(text),
            Want::FileMode => {
                (3..=4).contains(&text.len()) && text.chars().all(|c| ('0'..='7').contains(&c))
            }
            Want::FileName => {
                !text.is_empty() && !text.contains('/') && text != "." && text != ".."
            }
            _ => true,
        }
    }

    fn expects(&self) -> String {
        match self {
            Want::OneOf(choices) => format!(
                "one of {}",
                choices
                    .iter()
                    .map(|c| format!("`{c}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Want::Duration => "a whole number followed by s, m or h, such as `2m`".to_string(),
            Want::FileMode => "permissions in octal, such as `\"0600\"`".to_string(),
            Want::FileName => "a file name with no folder in it".to_string(),
            Want::Path(_) => "the path of a file".to_string(),
            _ => "text".to_string(),
        }
    }
}

/// Seconds in a duration written as a whole number and `s`, `m` or `h`.
fn is_duration(text: &str) -> bool {
    let Some(number) = text.strip_suffix(['s', 'm', 'h']) else {
        return false;
    };
    !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
        && number.parse::<u64>().is_ok_and(|n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Form {
        Form::Text(Some(value.to_string()))
    }

    #[test]
    fn durations() {
        for good in ["2m", "30s", "1h"] {
            assert!(is_duration(good), "{good}");
        }
        for bad in ["2", "m", "1.5m", "0s", "2d", "-1s"] {
            assert!(!is_duration(bad), "{bad}");
        }
    }

    #[test]
    fn wants_check_type_and_form() {
        assert_eq!(Want::Number.problem(&Form::Number(Some(3))), None);
        assert!(Want::Number.problem(&text("3")).is_some());
        assert!(Want::Text.problem(&Form::Number(Some(3))).is_some());
        assert_eq!(Want::Scalar.problem(&Form::Bool), None);
        assert!(
            Want::PortNumber
                .problem(&Form::Number(Some(70000)))
                .is_some()
        );
        assert_eq!(Want::PortNumber.problem(&Form::Number(None)), None);
        assert!(Want::Count.problem(&Form::Number(Some(0))).is_some());
        assert_eq!(Want::Count.problem(&Form::Number(Some(16))), None);
        assert!(Want::OneOf(&["a", "b"]).problem(&text("c")).is_some());
        assert!(Want::Duration.problem(&Form::Text(None)).is_some());
        assert!(Want::FileMode.problem(&text("0600")).is_none());
        assert!(Want::FileMode.problem(&text("rw")).is_some());
        assert!(Want::FileName.problem(&text("a/b")).is_some());
    }

    #[test]
    fn a_produced_value_passes_type_checks_but_not_a_file_read_before_start() {
        assert_eq!(Want::Number.problem(&Form::Unknown), None);
        assert!(
            Want::Path(PathUse::Read(Place::FileContent))
                .problem(&Form::Unknown)
                .is_some()
        );
    }
}
