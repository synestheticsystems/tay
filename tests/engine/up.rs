//! `tay up`: what it brings up, what it prints as it does, and how it ends.

mod took;

use std::fs;

use crate::harness::{Machine, kept_ports};

#[test]
fn units_that_are_made_come_up_ready_and_up_says_everything_asked_for_is() {
    let machine = Machine::new();
    let root = machine.stack("notes");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    for unit in [
        "notes-port",
        "notes-key",
        "notes-password",
        "signing",
        "notes-config",
    ] {
        assert!(
            up.changes().contains(&format!("{unit} starting making it")),
            "{}",
            up.stdout
        );
        assert!(up.last_change(unit).starts_with("ready "), "{}", up.stdout);
    }
    assert_eq!(
        up.last_change("notes"),
        "ready everything it needs is ready"
    );
    assert!(
        up.before_it_took()
            .ends_with("tay up: everything asked for is ready.\n"),
        "{}",
        up.stdout
    );
}

#[test]
fn a_unit_that_cannot_run_fails_saying_why_and_what_needs_it_waits_blocked() {
    let machine = Machine::new();
    let root = machine.stack("made-and-run");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    // `search` fails at once, and `jobs`, which needs it through `index`,
    // can no longer become ready: `tay up` returns then, naming the
    // failure, its reason, and every unit waiting on it. `site`, which
    // fails as soon, may have failed by then, and is named too.
    let cannot =
        "its program could not be started: `search-server`: No such file or directory (os error 2)";
    assert_eq!(up.last_change("search"), format!("failed {cannot}"));
    assert!(
        up.stdout.contains(&format!(
            "tay up: `search` failed: {cannot}\n  the last lines it wrote:\n"
        )),
        "{}",
        up.stdout
    );
    assert!(
        up.stdout.contains(
            " # attempt 1, command: could not be started: `search-server`: No such file or directory (os error 2)\n  waiting on it: `index`, `jobs`\n"
        ),
        "{}",
        up.stdout
    );
    // `index` needs `search`: it is never started, and says what failed.
    assert!(
        !up.changes()
            .iter()
            .any(|line| line.starts_with("index starting"))
    );
    // The daemon brings up the rest as far as it can.
    let status = machine.settled(&root);
    assert!(Machine::state(&status, "site-port").starts_with("ready "));
    assert_eq!(Machine::state(&status, "site-config"), "ready written");
    assert_eq!(
        Machine::state(&status, "site"),
        "failed its program could not be started: `serve`: No such file or directory (os error 2)"
    );
    assert_eq!(
        Machine::state(&status, "index"),
        "waiting blocked: `search` has failed"
    );
    assert_eq!(
        Machine::state(&status, "jobs"),
        "waiting blocked: `search`, `site` have failed"
    );
    // `notice` waits for `search` with `after`: ready or failed, it waits
    // no more, and does not depend on it.
    assert_eq!(Machine::state(&status, "notice"), "ready written");
}

#[test]
fn each_change_of_state_is_a_line_with_the_units_lined_up() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}", up.stderr);
    let lines: Vec<&str> = up.stdout.lines().collect();
    assert_eq!(lines[0], "  config-port  starting  making it");
    assert_eq!(lines[1], "  config       waiting   for `config-port`");
    assert!(
        lines[2].starts_with("  config-port  ready     2"),
        "{}",
        up.stdout
    );
    assert_eq!(lines[3], "  config       starting  making it");
    assert_eq!(lines[4], "  config       ready     written");
    assert_eq!(lines[5], "tay up: everything asked for is ready.");
    assert!(lines[6].starts_with("tay up: it took "), "{}", up.stdout);
    assert_eq!(lines.len(), 7);
}

#[test]
fn with_units_named_up_brings_up_those_and_what_they_need_and_leaves_the_rest() {
    let machine = Machine::new();
    let root = machine.stack("notes");
    let up = machine.at(&root, "up", &["notes-config"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let units: Vec<String> = up
        .changes()
        .iter()
        .map(|line| line.split(' ').next().unwrap().to_string())
        .collect();
    assert!(units.contains(&"notes-port".to_string()));
    assert!(!units.contains(&"signing".to_string()), "{}", up.stdout);
    assert!(!root.join(".tay/state/secrets/signing").exists());
    // What the environment is asked for is what it shows.
    let asked = ["notes-port", "notes-key", "notes-config"];
    let mut shown = machine.state_order(&root);
    shown.sort();
    let mut sorted = asked.to_vec();
    sorted.sort();
    assert_eq!(shown, sorted);
    // Then the rest, which leaves what is up as it is.
    let port = kept_ports(&root)["notes-port"].clone();
    let rest = machine.at(&root, "up", &["signing"]);
    assert_eq!(rest.code, 0, "{}", rest.stderr);
    let status = machine.at(&root, "status", &["--json"]).json();
    assert!(
        Machine::state(&status, "signing").starts_with("ready "),
        "{status}"
    );
    assert_eq!(machine.state_order(&root), ["signing"]);
    assert_eq!(kept_ports(&root)["notes-port"], port);
    assert!(root.join(".tay/state/files/config/notes.conf").exists());
    assert!(!root.join(".tay/state/secrets/notes-password").exists());
}

#[test]
fn errors_in_the_definitions_stop_up_before_anything_is_started() {
    let machine = Machine::new();
    let root = machine.stack("notes");
    fs::write(
        root.join("tay.yaml"),
        "format: 1\nstack: notes\nup: [nobody]\n",
    )
    .unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 2);
    assert_eq!(
        up.stderr,
        "tay.yaml:3:6: there is no unit `nobody`\ntay up: 1 error in the definitions; nothing was started.\n"
    );
    assert!(!root.join(".tay").exists(), "nothing is written");
    assert!(machine.daemon_pid(&root).is_none());
    let unknown = machine.at(&root, "up", &["nobody-either"]);
    assert_eq!(unknown.code, 2);
}

#[test]
fn a_tay_too_old_for_the_definitions_starts_nothing() {
    let machine = Machine::new();
    let root = machine.stack("notes");
    let definitions = fs::read_to_string(root.join("tay.yaml")).unwrap();
    fs::write(
        root.join("tay.yaml"),
        definitions.replace("format: 1", "format: 1\nrequires: \">=99.0.0\""),
    )
    .unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 3, "{}", up.stderr);
    assert!(!root.join(".tay").exists());
}

#[test]
fn up_with_json_prints_each_event_as_a_json_line_in_schema_1() {
    let machine = Machine::new();
    let root = machine.stack("made-and-run");
    let up = machine.at(&root, "up", &["--json"]);
    assert_eq!(up.code, 1);
    let lines: Vec<serde_json::Value> = up
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        lines
            .iter()
            .all(|line| line["schema"] == 1 && line["tay"].is_string())
    );
    assert_eq!(lines[0]["event"], "units");
    let search = lines
        .iter()
        .rfind(|line| line["event"] == "state" && line["unit"] == "search")
        .unwrap();
    assert_eq!(search["state"], "failed");
    assert!(search["at"].is_u64());
    let end = lines.last().unwrap();
    assert_eq!(end["event"], "up");
    assert_eq!(end["decision"], "failed");
    assert_eq!(end["failed"][0]["unit"], "search");
    assert_eq!(
        end["failed"][0]["waiting"],
        serde_json::json!(["index", "jobs"])
    );
}

#[test]
fn a_unit_with_watch_starts_as_any_other() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let up = machine.at(&root, "up", &["watcher"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(up.last_change("watcher"), "ready attempt 1, running");
    // Nothing is said of it but its states.
    assert!(!up.stdout.contains("tay up: `watcher`"), "{}", up.stdout);
}
