//! Whether this tay may read a stack's definitions at all. This is decided
//! before anything else in them is read: a tay that is too old must not
//! guess at fields it does not know.

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::syntax::fields::{self, Fields};
use crate::syntax::yaml::{Kind, Scalar};

/// The version of the definition format this tay reads.
pub const FORMAT: i64 = 1;

pub const THIS_TAY: &str = env!("CARGO_PKG_VERSION");

/// Checks `format` and `requires`. Errors in how they are written go to `d`;
/// a stack this tay is too old for comes back as `Err`, one line per reason.
pub fn gate(stack: &Fields<'_>, d: &mut Diagnostics) -> Result<(), Vec<Diagnostic>> {
    let mut too_old = Vec::new();
    if let Some(format) = stack.require("format", d) {
        match format.kind {
            Kind::Scalar(Scalar::Int(FORMAT)) => {}
            Kind::Scalar(Scalar::Int(other)) => too_old.push(Diagnostic::new(
                format.loc,
                format!(
                    "these definitions are written in format {other}, and this tay ({THIS_TAY}) reads format {FORMAT} only; a tay that reads format {other} is needed"
                ),
            )),
            _ => d.error(
                format.loc,
                format!(
                    "`format` is the version of the definition format, a whole number; this is {}",
                    format.describe()
                ),
            ),
        }
    }
    if let Some(requires) = stack.get("requires")
        && let Some(text) = fields::text(requires, "`requires`", d)
    {
        match text.strip_prefix(">=").and_then(parse_version) {
            None => d.error(
                requires.loc,
                format!(
                    "`requires` is written `>=MAJOR.MINOR.PATCH`, the oldest tay that may read these definitions; this is `{text}`"
                ),
            ),
            Some(needed) => {
                let this = parse_version(THIS_TAY).expect("this tay's own version is MAJOR.MINOR.PATCH");
                if this < needed {
                    too_old.push(Diagnostic::new(
                        requires.loc,
                        format!(
                            "these definitions need tay {}.{}.{} or newer; this is tay {THIS_TAY}",
                            needed.0, needed.1, needed.2
                        ),
                    ));
                }
            }
        }
    }
    if too_old.is_empty() {
        Ok(())
    } else {
        Err(too_old)
    }
}

/// A version of three numbers, as `requires` and tay write it.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.').map(|part| {
        (!part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
            .then(|| part.parse::<u64>().ok())
            .flatten()
    });
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert_eq!(parse_version("0.10.2"), Some((0, 10, 2)));
        assert!(parse_version("0.10.0") > parse_version("0.9.0"));
        for bad in ["1.2", "1.2.3.4", "1.x.3", "", "1..3"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }
}
