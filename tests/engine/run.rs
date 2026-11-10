//! `tay run ACTION [NAME=VALUE...]`: an action is run with its arguments,
//! checked before anything runs, after what it needs is brought up; what
//! it writes is printed as it writes it, and then each value it produced,
//! by name, a secret one never; `tay run` exits as the action did, and
//! nothing the action gave is kept.

use std::fs;
use std::path::Path;

use crate::harness::Machine;

/// Every file under `folder`, with what it holds, as text.
fn every_file(folder: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(every_file(&path));
        } else if let Ok(text) = fs::read(&path) {
            let text = String::from_utf8_lossy(&text).into_owned();
            files.push((path.display().to_string(), text));
        }
    }
    files
}

#[test]
fn run_prints_what_the_action_writes_and_the_values_it_produced_and_keeps_none() {
    let machine = Machine::new();
    let root = machine.stack("run");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let run = machine.at(&root, "run", &["mint", "user=alice"]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    // What the action wrote, then its values by name, on standard output;
    // tay's own lines on standard error.
    assert_eq!(
        run.stdout,
        "minting for alice\nvalid for 8 hours\nsession  s-alice-8\ntoken    <secret>\n"
    );
    assert!(
        run.stderr
            .ends_with("tay run: the action is done.\ntay run: the action produced:\n"),
        "{}",
        run.stderr
    );
    // An argument given replaces its default.
    let longer = machine.at(&root, "run", &["mint", "user=bob", "hours=12"]);
    assert_eq!(longer.code, 0, "{}{}", longer.stdout, longer.stderr);
    assert!(
        longer
            .stdout
            .starts_with("minting for bob\nvalid for 12 hours\n")
    );
    // Neither the values nor the secret are kept, nor printed anywhere.
    for (path, text) in every_file(&root.join(".tay")) {
        assert!(!text.contains("t0ken-for"), "{path} holds the secret");
        assert!(!text.contains("s-alice-8"), "{path} keeps a value");
    }
    for printed in [&run.stdout, &run.stderr, &longer.stdout, &longer.stderr] {
        assert!(!printed.contains("t0ken-for"), "{printed}");
    }
    // The action leaves the plan once it has run.
    let status = machine.at(&root, "status", &["--json"]).json();
    let units: Vec<&str> = status["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|unit| unit["unit"].as_str().unwrap())
        .collect();
    assert_eq!(units, ["web"]);
}

#[test]
fn run_exits_as_the_action_did() {
    let machine = Machine::new();
    let root = machine.stack("run");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let run = machine.at(&root, "run", &["fail"]);
    assert_eq!(run.code, 3, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, "failing with 3\n");
    assert!(
        run.stderr
            .contains("tay run: `fail` failed: attempt 1 ran and exited with 3 at "),
        "{}",
        run.stderr
    );
    let other = machine.at(&root, "run", &["fail", "code=42"]);
    assert_eq!(other.code, 42, "{}{}", other.stdout, other.stderr);
    // With --json, each line the action wrote is an event, and so is the
    // end, with how its program exited.
    let json = machine.at(&root, "run", &["fail", "code=5", "--json"]);
    assert_eq!(json.code, 5);
    let events: Vec<serde_json::Value> = json
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        events
            .iter()
            .any(|event| event["event"] == "output" && event["line"] == "failing with 5"),
        "{}",
        json.stdout
    );
    let end = events.last().unwrap();
    assert_eq!(
        (&end["event"], &end["decision"], &end["exit_status"]),
        (
            &serde_json::json!("run"),
            &serde_json::json!("failed"),
            &serde_json::json!(5)
        )
    );
}

#[test]
fn run_prints_each_line_the_action_writes_while_it_runs() {
    let machine = Machine::new();
    let root = machine.stack("run");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let mut run = machine.spawn(&root, "run", &["slow"]);
    // The action goes on only once the test has read its first line.
    run.until("waiting for go");
    fs::write(root.join(".tay/state/go"), "").unwrap();
    let ran = run.finish();
    assert_eq!(ran.code, 0, "{}{}", ran.stdout, ran.stderr);
    assert_eq!(ran.stdout, "waiting for go\nwent\n");
}

#[test]
fn run_brings_up_what_the_action_needs_first() {
    let machine = Machine::new();
    let root = machine.stack("run");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    // Nothing in `up` needs the worker, which is brought up for the seed.
    let run = machine.at(&root, "run", &["seed"]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, "the worker is up\n");
    assert!(run.stderr.contains("  worker  ready"), "{}", run.stderr);
}

#[test]
fn an_action_or_argument_that_does_not_fit_is_an_error_before_anything_runs() {
    let machine = Machine::new();
    let root = machine.stack("run");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    for (args, error) in [
        (
            &["mint"][..],
            "tay run mint: `mint` must be given the argument `user`, which has no default",
        ),
        (
            &["mint", "user=alice", "colour=red"],
            "tay run mint colour: `mint` declares no argument `colour`: it declares `user`, `hours`",
        ),
        (
            &["mint", "user=alice", "hours=many"],
            "tay run mint hours: `hours` is a whole number, as its default is; `many` is not",
        ),
        (
            &["mint", "user=alice", "user=bob"],
            "tay run mint user: `user` is given twice",
        ),
        (
            &["web"],
            "tay run web: `web` is not an action: `tay run` runs a unit that has `action: true`",
        ),
        (&["nobody"], "tay run nobody: there is no unit `nobody`"),
    ] {
        let run = machine.at(&root, "run", args);
        assert_eq!(run.code, 2, "{args:?}: {}{}", run.stdout, run.stderr);
        assert_eq!(
            run.stderr,
            format!("{error}\ntay run: 1 error in the definitions; nothing was run.\n"),
            "{args:?}"
        );
    }
    assert!(!root.join(".tay/logs/mint").exists(), "nothing ran");
}

#[test]
fn run_on_an_environment_that_is_down_is_refused() {
    let machine = Machine::new();
    let root = machine.stack("run");
    let never = machine.at(&root, "run", &["fail"]);
    assert_eq!(
        (never.code, never.stderr.as_str()),
        (
            4,
            "tay run: the environment is down: `tay up` brings it up\n"
        )
    );
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(machine.at(&root, "down", &[]).code, 0);
    let down = machine.at(&root, "run", &["fail"]);
    assert_eq!(down.code, 4, "{}{}", down.stdout, down.stderr);
    assert!(!root.join(".tay/logs/fail").exists(), "nothing ran");
    assert!(machine.daemon(&root).is_none());
}
