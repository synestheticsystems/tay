//! The line a change that brings units up ends with: how long it took, and
//! which units it started took longest, so that a slow bring-up shows where
//! its time went.

use serde_json::{Value, json};

use crate::engine::{State, Took};

/// A unit is named when it took at least this many seconds.
const NAMED_FROM: f64 = 1.0;

/// At most this many units are named.
const NAMED_AT_MOST: usize = 5;

/// The line as `command` says it of a change that took `seconds`: with
/// each unit in `took`, which is the longest first, that took a second or
/// more, five at most, and of one still under way, that it is.
pub fn line(command: &str, seconds: f64, took: &[Took]) -> String {
    let mut line = format!("{command}: it took {}", written(seconds));
    let named = (took.iter())
        .filter(|took| took.seconds >= NAMED_FROM)
        .take(NAMED_AT_MOST);
    for (index, took) in named.enumerate() {
        let (before, verb) = match index {
            0 => ("; ", " took"),
            _ => (", ", ""),
        };
        let still = match took.state {
            State::Ready | State::Failed => String::new(),
            under_way => format!(" (still {})", under_way.word()),
        };
        line.push_str(&format!(
            "{before}`{}`{verb} {}{still}",
            took.unit,
            written(took.seconds)
        ));
    }
    line.push_str(".\n");
    line
}

/// Adds to the JSON line of what a change came to how long it took,
/// `seconds`, and under `took` each unit it started, the longest first,
/// with its seconds and the state it came to.
pub fn add_to(line: &mut Value, seconds: f64, took: &[Took]) {
    line["seconds"] = json!(seconds);
    line["took"] = (took.iter())
        .map(|took| {
            json!({
                "unit": took.unit,
                "seconds": took.seconds,
                "state": took.state.word(),
            })
        })
        .collect();
}

/// `seconds` as a person reads them: with one decimal below ten, and whole
/// from ten on.
fn written(seconds: f64) -> String {
    match (seconds * 10.0).round() < 100.0 {
        true => format!("{seconds:.1}s"),
        false => format!("{seconds:.0}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn took(unit: &str, seconds: f64, state: State) -> Took {
        Took {
            unit: unit.to_string(),
            seconds,
            state,
        }
    }

    #[test]
    fn seconds_have_one_decimal_below_ten_and_none_from_ten_on() {
        assert_eq!(written(0.04), "0.0s");
        assert_eq!(written(1.24), "1.2s");
        assert_eq!(written(9.94), "9.9s");
        // What would be shown as ten is shown whole.
        assert_eq!(written(9.96), "10s");
        assert_eq!(written(12.4), "12s");
        assert_eq!(written(127.6), "128s");
    }

    #[test]
    fn the_line_names_up_to_five_units_that_took_a_second_or_more() {
        let took = [
            took("build", 31.6, State::Ready),
            took("database", 12.0, State::Starting),
            took("migrate", 4.24, State::Failed),
            took("seed", 3.0, State::Ready),
            took("queue", 2.0, State::Waiting),
            took("cache", 1.5, State::Ready),
            took("port", 0.999, State::Ready),
        ];
        assert_eq!(
            line("tay up", 40.4, &took),
            "tay up: it took 40s; `build` took 32s, `database` 12s (still starting), `migrate` 4.2s, `seed` 3.0s, `queue` 2.0s (still waiting).\n"
        );
        assert_eq!(
            line("tay restart", 1.234, &took[5..]),
            "tay restart: it took 1.2s; `cache` took 1.5s.\n"
        );
        assert_eq!(line("tay up", 0.42, &took[6..]), "tay up: it took 0.4s.\n");
    }

    #[test]
    fn with_json_the_line_carries_the_seconds_and_every_unit_started() {
        let mut line = json!({ "event": "up", "decision": "ready" });
        let took = [
            took("seed", 3.0, State::Ready),
            took("port", 0.004, State::Ready),
        ];
        add_to(&mut line, 3.211, &took);
        assert_eq!(
            line,
            json!({
                "event": "up",
                "decision": "ready",
                "seconds": 3.211,
                "took": [
                    { "unit": "seed", "seconds": 3.0, "state": "ready" },
                    { "unit": "port", "seconds": 0.004, "state": "ready" },
                ],
            })
        );
    }
}
