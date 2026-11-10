//! What a plain (unquoted) scalar means, by the YAML 1.2 core schema.
//!
//! Only these forms are anything but text, so `no`, `yes`, `on` and `off`
//! stay text, and `0600` is the whole number six hundred (an octal number is
//! written `0o600`).

use super::Scalar;

pub fn resolve_plain(text: &str) -> Scalar {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return Scalar::Null,
        "true" | "True" | "TRUE" => return Scalar::Bool(true),
        "false" | "False" | "FALSE" => return Scalar::Bool(false),
        _ => {}
    }
    if let Some(number) = integer(text) {
        return number;
    }
    if is_float(text) {
        return Scalar::Float;
    }
    Scalar::Str(super::Str {
        text: text.to_string(),
        placement: super::Placement::AtStart,
    })
}

fn integer(text: &str) -> Option<Scalar> {
    let parsed = if let Some(octal) = text.strip_prefix("0o") {
        all_digits(octal, 8).then(|| i64::from_str_radix(octal, 8))
    } else if let Some(hex) = text.strip_prefix("0x") {
        all_digits(hex, 16).then(|| i64::from_str_radix(hex, 16))
    } else {
        let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
        all_digits(digits, 10).then(|| text.parse::<i64>())
    }?;
    Some(match parsed {
        Ok(value) => Scalar::Int(value),
        Err(_) => Scalar::TooLarge,
    })
}

fn all_digits(text: &str, radix: u32) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_digit(radix))
}

/// `[-+]? ( \. [0-9]+ | [0-9]+ ( \. [0-9]* )? ) ( [eE] [-+]? [0-9]+ )?`, and the
/// spellings of infinity and not-a-number.
fn is_float(text: &str) -> bool {
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") || matches!(text, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (&unsigned[..at], Some(&unsigned[at + 1..])),
        None => (unsigned, None),
    };
    let mantissa_ok = match mantissa.split_once('.') {
        Some((whole, fraction)) => {
            (whole.is_empty() && all_digits(fraction, 10))
                || (all_digits(whole, 10) && fraction.chars().all(|c| c.is_ascii_digit()))
        }
        None => all_digits(mantissa, 10),
    };
    let exponent_ok =
        exponent.is_none_or(|e| all_digits(e.strip_prefix(['-', '+']).unwrap_or(e), 10));
    mantissa_ok && exponent_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_text(text: &str) -> bool {
        matches!(resolve_plain(text), Scalar::Str(s) if s.text == text)
    }

    #[test]
    fn words_that_yaml_1_1_made_booleans_stay_text() {
        for word in ["no", "yes", "on", "off", "No", "YES", "y", "n"] {
            assert!(is_text(word), "{word} should be text");
        }
    }

    #[test]
    fn core_schema_booleans_and_nulls() {
        assert_eq!(resolve_plain("true"), Scalar::Bool(true));
        assert_eq!(resolve_plain("FALSE"), Scalar::Bool(false));
        assert_eq!(resolve_plain("~"), Scalar::Null);
        assert_eq!(resolve_plain(""), Scalar::Null);
    }

    #[test]
    fn integers_in_three_bases_and_a_leading_zero_is_decimal() {
        assert_eq!(resolve_plain("0600"), Scalar::Int(600));
        assert_eq!(resolve_plain("0o600"), Scalar::Int(384));
        assert_eq!(resolve_plain("0x1F"), Scalar::Int(31));
        assert_eq!(resolve_plain("-12"), Scalar::Int(-12));
        assert_eq!(resolve_plain("99999999999999999999"), Scalar::TooLarge);
    }

    #[test]
    fn floats_are_recognised_so_they_can_be_refused() {
        for text in ["1.5", ".5", "1.", "1e3", "-2.5E-3", ".inf", "-.Inf", ".nan"] {
            assert_eq!(resolve_plain(text), Scalar::Float, "{text}");
        }
        assert!(is_text("1.2.3"));
        assert!(is_text("1e"));
        assert!(is_text("v1.0"));
    }
}
