//! How `tay up` ends, whatever it came to: with how long it took, and each
//! unit it started that took a second or more, the longest first; with
//! `--json`, with every unit it started.

use serde_json::Value;

use crate::harness::Machine;
use crate::support::Run;

/// More than any of these units takes to come up, however loaded the
/// machine: what a time is checked to be below.
const ALLOWED: f64 = 20.0;

/// What the last line of `up` says: the seconds of the whole, and each unit
/// it names, with its seconds and anything said after them.
fn closing(up: &Run) -> (f64, Vec<(String, f64, String)>) {
    let last = up.stdout.lines().last().unwrap_or_default();
    let said = (last.strip_prefix("tay up: it took "))
        .and_then(|said| said.strip_suffix('.'))
        .unwrap_or_else(|| panic!("the last line says how long it took:\n{}", up.stdout));
    let (whole, named) = said.split_once("; ").unwrap_or((said, ""));
    let named = (named.split(", ").filter(|one| !one.is_empty()))
        .map(|one| {
            let (unit, rest) = (one.strip_prefix('`'))
                .and_then(|one| one.split_once('`'))
                .unwrap_or_else(|| panic!("`{one}` names a unit"));
            let rest = rest.strip_prefix(" took").unwrap_or(rest).trim_start();
            let (time, after) = rest.split_once(' ').unwrap_or((rest, ""));
            (unit.to_string(), seconds(time), after.to_string())
        })
        .collect();
    (seconds(whole), named)
}

/// The seconds `written` gives, as `2.0s` or `12s`.
fn seconds(written: &str) -> f64 {
    (written.strip_suffix('s'))
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("`{written}` is a number of seconds"))
}

/// The line `tay up --json` ends with.
fn closing_json(up: &Run) -> Value {
    let last = up.stdout.lines().last().unwrap_or_default();
    serde_json::from_str(last).unwrap_or_else(|_| panic!("a JSON line ends it:\n{}", up.stdout))
}

#[test]
fn up_ends_naming_the_units_that_took_a_second_or_more_the_longest_first() {
    let machine = Machine::new();
    let root = machine.stack("timings");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(
        up.before_it_took()
            .ends_with("tay up: everything asked for is ready.\n"),
        "{}",
        up.stdout
    );
    // `slower` sleeps two seconds and `slow` a little over one, and
    // `instant` takes no time at all, so it is not named.
    let (whole, named) = closing(&up);
    let units: Vec<&str> = named.iter().map(|(unit, ..)| unit.as_str()).collect();
    assert_eq!(units, ["slower", "slow"], "{}", up.stdout);
    let (slower, slow) = (named[0].1, named[1].1);
    assert!((2.0..ALLOWED).contains(&slower), "{}", up.stdout);
    assert!((1.1..ALLOWED).contains(&slow), "{}", up.stdout);
    assert!(whole >= slower, "{}", up.stdout);
    assert!(
        named.iter().all(|(.., after)| after.is_empty()),
        "{}",
        up.stdout
    );
    // A command with no check runs at every `tay up`, so it is started
    // again, and with `--json` each unit started is under `took`.
    let again = machine.at(&root, "up", &["--json"]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    let end = closing_json(&again);
    assert_eq!(end["event"], "up");
    assert_eq!(end["decision"], "ready");
    let took = end["took"].as_array().unwrap();
    let units: Vec<&str> = took
        .iter()
        .map(|took| took["unit"].as_str().unwrap())
        .collect();
    assert_eq!(units, ["slower", "slow", "instant"], "{end}");
    assert!(took.iter().all(|took| took["state"] == "ready"), "{end}");
    let seconds: Vec<f64> = (took.iter())
        .map(|took| took["seconds"].as_f64().unwrap())
        .collect();
    assert!((2.0..ALLOWED).contains(&seconds[0]), "{end}");
    assert!((1.1..ALLOWED).contains(&seconds[1]), "{end}");
    assert!((0.0..1.0).contains(&seconds[2]), "{end}");
    assert!(end["seconds"].as_f64().unwrap() >= seconds[0], "{end}");
}

#[test]
fn up_with_nothing_to_start_ends_with_the_whole_time_alone() {
    let machine = Machine::new();
    let root = machine.stack("timings");
    let first = machine.at(&root, "up", &["steady"]);
    assert_eq!(first.code, 0, "{}{}", first.stdout, first.stderr);
    // `steady` runs, and a process that runs is joined, not started.
    let again = machine.at(&root, "up", &[]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    let (whole, named) = closing(&again);
    assert!((0.0..ALLOWED).contains(&whole), "{}", again.stdout);
    assert!(named.is_empty(), "{}", again.stdout);
    let json = machine.at(&root, "up", &["--json"]);
    assert_eq!(json.code, 0, "{}{}", json.stdout, json.stderr);
    let end = closing_json(&json);
    assert!(end["seconds"].as_f64().is_some(), "{end}");
    assert_eq!(end["took"], serde_json::json!([]), "{end}");
}

#[test]
fn a_failed_up_ends_with_how_long_it_took_too() {
    let machine = Machine::new();
    let root = machine.stack("timings");
    let up = machine.at(&root, "up", &["failing", "lasting"]);
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    assert!(
        up.before_it_took().contains("tay up: `failing` failed: "),
        "{}",
        up.stdout
    );
    // `failing` failed after a little over a second, while `lasting`,
    // started with it, goes on: it is named with the time it had taken
    // when `tay up` ended, and as still starting.
    let (whole, named) = closing(&up);
    let named_as = |unit: &str| {
        let found = named.iter().find(|(named, ..)| named == unit);
        found.unwrap_or_else(|| panic!("`{unit}` is named:\n{}", up.stdout))
    };
    let (_, failing, after) = named_as("failing");
    assert!((1.1..ALLOWED).contains(failing), "{}", up.stdout);
    assert!(after.is_empty(), "{}", up.stdout);
    let (_, lasting, after) = named_as("lasting");
    assert!((1.0..ALLOWED).contains(lasting), "{}", up.stdout);
    assert_eq!(after, "(still starting)", "{}", up.stdout);
    assert!(whole >= *failing, "{}", up.stdout);
}
