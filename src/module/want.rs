//! What a value must be where it is used, checked against what is known of it.

use crate::expr::{Expr, ExprKind, Piece};

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
    OneOf(&'static [&'static str]),
    /// A whole number followed by `s`, `m` or `h`.
    Duration,
    /// Permissions written as octal text, such as `"0600"`.
    FileMode,
    /// A file's name, with no folder in it.
    FileName,
    /// A path to a file whose text is read before start and has its
    /// references filled in; the text goes to `Place`.
    TemplateFile(Place),
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
    /// The form of a value as written, or `None` when a reference decides it.
    pub fn of_written(expr: &Expr) -> Option<Form> {
        Some(match &expr.kind {
            ExprKind::Number(n) => Form::Number(Some(*n)),
            ExprKind::Bool(_) => Form::Bool,
            ExprKind::Text(pieces) => Form::Text(
                pieces
                    .iter()
                    .map(|piece| match piece {
                        Piece::Lit(text) => Some(text.as_str()),
                        Piece::Ref(_) => None,
                    })
                    .collect(),
            ),
            ExprKind::Ref(_) => return None,
        })
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
            (Want::TemplateFile(_), Form::Unknown) => Some(
                "must be known before start, because tay reads the file then; this is produced by running"
                    .to_string(),
            ),
            (_, Form::Unknown) | (Want::Scalar, _) => None,
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
            Want::Duration => parse_duration(text).is_some(),
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
            Want::TemplateFile(_) => "the path of a file".to_string(),
            _ => "text".to_string(),
        }
    }
}

/// Seconds in a duration written as a whole number and `s`, `m` or `h`.
pub fn parse_duration(text: &str) -> Option<u64> {
    let unit = text.chars().last()?;
    let scale = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        _ => return None,
    };
    let number = &text[..text.len() - 1];
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value: u64 = number.parse().ok()?;
    (value > 0).then(|| value.saturating_mul(scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Form {
        Form::Text(Some(value.to_string()))
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("2m"), Some(120));
        assert_eq!(parse_duration("30s"), Some(30));
        assert_eq!(parse_duration("1h"), Some(3600));
        for bad in ["2", "m", "1.5m", "0s", "2d", "-1s"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
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
            Want::TemplateFile(Place::PrivateFile)
                .problem(&Form::Unknown)
                .is_some()
        );
    }
}
