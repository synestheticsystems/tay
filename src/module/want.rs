//! What a value must be where it is used, checked against what is known of it.

use std::fmt;

use crate::diagnostic::{Diagnostics, quoted_list};
use crate::syntax::expr::{self, Expr, ExprKind, Type};
use crate::syntax::yaml::Node;
use crate::value::{Placeholder, Value};

use super::settings::Destination;

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
    /// A whole number from 1 to this: how many of something there may be
    /// no more of than that.
    CountUpTo(i64),
    OneOf(&'static [&'static str]),
    /// A whole number followed by `s`, `m` or `h`.
    Duration,
    /// Permissions written as octal text, such as `"0600"`.
    FileMode,
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
    /// the state, log or shared folder, where tay makes it, or is built from
    /// a value produced by running.
    Mounted,
    /// A file whose text is read before start and has its references
    /// filled in. The value used is the text, which ends up where the
    /// destination says.
    Read(Destination),
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
    /// An action's argument with no default: it is known when the action is
    /// asked for, and its type is whatever it is asked with.
    Asked,
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

    /// The form of a resolved value. A placeholder's exact form is not known,
    /// even when the placeholder is a secret whose value is.
    pub fn of(value: &Value) -> Form {
        match value {
            Value::Text(_) => Form::Text(value.exact_text()),
            Value::Number(n) => Form::Number(Some(*n)),
            Value::Bool(_) => Form::Bool,
            Value::Placeholder(Placeholder::Arg { ty: None, .. }) => Form::Asked,
            Value::Placeholder(placeholder) => match placeholder.ty() {
                Some(Type::Text) => Form::Text(None),
                Some(Type::Number) => Form::Number(None),
                Some(Type::Bool) => Form::Bool,
                None => Form::Unknown,
            },
        }
    }

    fn describe(&self) -> &'static str {
        match self {
            Form::Text(_) => "text",
            Form::Number(_) => "a whole number",
            Form::Bool => "true or false",
            Form::Unknown => "produced by running",
            Form::Asked => "an action's argument",
        }
    }
}

impl Want {
    /// Reads a value written where this is wanted. Its written form is
    /// checked now; what a reference in it brings is checked once it is
    /// resolved.
    pub fn read(self, node: &Node, d: &mut Diagnostics) -> Option<Expr> {
        let expr = expr::from_node(node, d)?;
        if let Some(problem) = Form::of_written(&expr).and_then(|form| self.problem(&form)) {
            d.error(node.loc, format!("this {problem}"));
            return None;
        }
        Some(expr)
    }

    /// What is wrong with a value of this form here, if anything. A value
    /// produced by running passes every check that its type would decide,
    /// since its type is not known before start, and none that its exact
    /// form would.
    pub fn problem(&self, form: &Form) -> Option<String> {
        let wrong = |wanted: &str| Some(format!("must be {wanted}; this is {}", form.describe()));
        match (self, form) {
            (Want::Path(PathUse::Read(_)), Form::Unknown) => Some(
                "must be known before start, because tay reads the file then; this is produced by running"
                    .to_string(),
            ),
            (want, Form::Unknown) if want.checks_exact_form() => Some(format!(
                "must be known before start ({}); this is produced by running",
                want.expects()
            )),
            (want, Form::Asked) if want.checks_exact_form() || matches!(want, Want::Path(PathUse::Read(_))) => {
                Some(format!(
                    "must be written out in full ({}); this holds a value not known until start",
                    want.expects()
                ))
            }
            (_, Form::Unknown | Form::Asked) | (Want::Scalar, _) => None,
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
            (Want::CountUpTo(most), Form::Number(Some(n))) if !(1..=*most).contains(n) => {
                Some(format!("must be {}; this is {n}", self.expects()))
            }
            (Want::Count | Want::CountUpTo(_), Form::Number(Some(_))) => None,
            (Want::Count | Want::CountUpTo(_), Form::Number(None)) => Some(format!(
                "must be written out in full ({}); this holds a value not known until start",
                self.expects()
            )),
            (Want::Count | Want::CountUpTo(_), _) => wrong("a whole number"),
            (_, Form::Number(_) | Form::Bool) => wrong("text"),
            (want, Form::Text(None)) => Some(format!(
                "must be written out in full ({}); this holds a value not known until start",
                want.expects()
            )),
            (want, Form::Text(Some(text))) => (!want.accepts(text))
                .then(|| format!("must be {}; this is `{text}`", want.expects())),
        }
    }

    /// Whether the value must be known exactly to be checked, rather than
    /// only its type.
    fn checks_exact_form(&self) -> bool {
        matches!(
            self,
            Want::Count | Want::CountUpTo(_) | Want::OneOf(_) | Want::Duration | Want::FileMode
        )
    }

    fn accepts(&self, text: &str) -> bool {
        match self {
            Want::OneOf(choices) => choices.contains(&text),
            Want::Duration => Duration::parse(text).is_some(),
            Want::FileMode => {
                (3..=4).contains(&text.len()) && text.chars().all(|c| ('0'..='7').contains(&c))
            }
            _ => true,
        }
    }

    fn expects(&self) -> String {
        match self {
            Want::OneOf(choices) => format!("one of {}", quoted_list(*choices)),
            Want::Duration => "a whole number followed by s, m or h, such as `2m`".to_string(),
            Want::FileMode => "permissions in octal, such as `\"0600\"`".to_string(),
            Want::Path(_) => "the path of a file".to_string(),
            Want::Count => "a whole number above 0".to_string(),
            Want::CountUpTo(most) => format!("a whole number from 1 to {most}"),
            _ => "text".to_string(),
        }
    }
}

/// A length of time as a definition writes it: a whole number above 0, and
/// `s`, `m` or `h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Duration {
    pub count: u64,
    pub unit: TimeUnit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeUnit {
    Seconds,
    Minutes,
    Hours,
}

impl Duration {
    pub fn parse(text: &str) -> Option<Duration> {
        let unit = match text.chars().last()? {
            's' => TimeUnit::Seconds,
            'm' => TimeUnit::Minutes,
            'h' => TimeUnit::Hours,
            _ => return None,
        };
        let number = &text[..text.len() - 1];
        if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let count = number.parse::<u64>().ok().filter(|n| *n > 0)?;
        Some(Duration { count, unit })
    }
}

impl Duration {
    /// The length of time it is.
    pub fn length(self) -> std::time::Duration {
        let seconds = match self.unit {
            TimeUnit::Seconds => 1,
            TimeUnit::Minutes => 60,
            TimeUnit::Hours => 3600,
        };
        std::time::Duration::from_secs(self.count.saturating_mul(seconds))
    }
}

/// As it is written: `2m`.
impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let unit = match self.unit {
            TimeUnit::Seconds => 's',
            TimeUnit::Minutes => 'm',
            TimeUnit::Hours => 'h',
        };
        write!(f, "{}{unit}", self.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Form {
        Form::Text(Some(value.to_string()))
    }

    #[test]
    fn durations_are_read_and_written_as_a_number_and_a_unit() {
        for good in ["2m", "30s", "1h", "120s"] {
            assert_eq!(Duration::parse(good).unwrap().to_string(), good);
        }
        assert_eq!(
            Duration::parse("90s"),
            Some(Duration {
                count: 90,
                unit: TimeUnit::Seconds
            })
        );
        for bad in ["2", "m", "1.5m", "0s", "2d", "-1s", "", "2é"] {
            assert_eq!(Duration::parse(bad), None, "{bad}");
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
        assert!(Want::Count.problem(&Form::Number(None)).is_some());
        assert_eq!(Want::CountUpTo(8).problem(&Form::Number(Some(8))), None);
        assert_eq!(
            Want::CountUpTo(8).problem(&Form::Number(Some(9))),
            Some("must be a whole number from 1 to 8; this is 9".to_string())
        );
        assert!(Want::CountUpTo(8).problem(&Form::Number(Some(0))).is_some());
        assert!(Want::CountUpTo(8).problem(&Form::Number(None)).is_some());
        assert!(Want::OneOf(&["a", "b"]).problem(&text("c")).is_some());
        assert!(Want::Duration.problem(&Form::Text(None)).is_some());
        assert!(Want::FileMode.problem(&text("0600")).is_none());
        assert!(Want::FileMode.problem(&text("rw")).is_some());
    }

    #[test]
    fn a_produced_value_passes_type_checks_but_not_one_of_its_exact_form() {
        assert_eq!(Want::Number.problem(&Form::Unknown), None);
        assert_eq!(Want::Text.problem(&Form::Unknown), None);
        assert!(
            Want::Path(PathUse::Read(Destination::FileContent))
                .problem(&Form::Unknown)
                .is_some()
        );
        for want in [
            Want::Count,
            Want::CountUpTo(8),
            Want::Duration,
            Want::OneOf(&["a"]),
            Want::FileMode,
        ] {
            assert_eq!(
                want.problem(&Form::Unknown),
                Some(format!(
                    "must be known before start ({}); this is produced by running",
                    want.expects()
                ))
            );
        }
    }

    #[test]
    fn an_argument_with_no_default_passes_type_checks_but_not_one_of_its_exact_form() {
        assert_eq!(Want::Number.problem(&Form::Asked), None);
        assert_eq!(Want::Text.problem(&Form::Asked), None);
        assert_eq!(
            Want::Duration.problem(&Form::Asked),
            Some(
                "must be written out in full (a whole number followed by s, m or h, such as `2m`); this holds a value not known until start"
                    .to_string()
            )
        );
        assert!(
            Want::Path(PathUse::Read(Destination::FileContent))
                .problem(&Form::Asked)
                .is_some()
        );
    }
}
