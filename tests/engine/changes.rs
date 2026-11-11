//! When something changes: at `tay up` on an environment that is up, each
//! unit that runs is compared with what it was started from, its settings
//! and the files it uses that tay knows of, a file tay does not overwrite
//! by its being there alone. One that differs is started again, and
//! `tay up` says what changed, by name, never showing a secret; one that
//! only receives a value from a unit that changed is started again only if
//! the value it receives changed; everything else is left running, and said
//! once as it is.

use std::path::Path;
use std::time::Duration;

use crate::harness::{Machine, child, eventually, left_running, log};

/// Waits until `unit`'s log has a line with `said`.
fn said(root: &Path, unit: &str, said: &str) {
    assert!(
        eventually(Duration::from_secs(20), || log(root, unit).contains(said)),
        "`{unit}` said `{said}`: {}",
        log(root, unit)
    );
}

/// The line `tay up` prints of `unit` started again because `changed`
/// changed.
fn again(unit: &str, changed: &str) -> String {
    format!("{unit} starting {changed} changed, so it is started again")
}

#[test]
fn a_unit_whose_setting_changed_is_started_again_and_one_given_an_unchanged_value_is_left() {
    let machine = Machine::new();
    let root = machine.stack("changes");
    let up = machine.at(&root, "up", &["web", "follower", "caller"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    said(&root, "caller", "| calls http://");
    let caller = child(&root, "caller");
    // Nothing changed: no unit that runs is started again. The port it
    // uses is made again, as a made unit is at every `tay up`, and kept.
    let same = machine.at(&root, "up", &[]);
    assert_eq!(same.code, 0, "{}{}", same.stdout, same.stderr);
    for unit in ["web", "follower", "caller"] {
        left_running(&same, unit);
    }
    let changed = machine.at(&root, "up", &["--set", "settings.mode=two"]);
    assert_eq!(changed.code, 0, "{}{}", changed.stdout, changed.stderr);
    let changes = changed.changes();
    // The web's own setting changed; the follower is given the web's mode,
    // which changed with it; the caller is given the web's address, which
    // did not.
    for unit in ["web", "follower"] {
        assert!(
            changes.contains(&again(unit, "`env.MODE`")),
            "{}",
            changed.stdout
        );
    }
    left_running(&changed, "caller");
    said(&root, "web", "| mode two");
    said(&root, "follower", "| follows two");
    assert_eq!(child(&root, "caller"), caller);
}

#[test]
fn a_unit_that_uses_a_file_tay_writes_is_started_again_when_its_content_changed() {
    let machine = Machine::new();
    let root = machine.stack("changes");
    assert_eq!(machine.at(&root, "up", &["reader"]).code, 0);
    said(&root, "reader", "| reads hello");
    let changed = machine.at(&root, "up", &["--set", "settings.greeting=bye"]);
    assert_eq!(changed.code, 0, "{}{}", changed.stdout, changed.stderr);
    assert!(
        changed
            .changes()
            .contains(&again("reader", "the file `conf`")),
        "{}",
        changed.stdout
    );
    said(&root, "reader", "| reads bye");
}

#[test]
fn a_unit_that_uses_a_file_tay_does_not_overwrite_is_left_running_when_the_file_is_rewritten() {
    let machine = Machine::new();
    let root = machine.stack("changes");
    assert_eq!(machine.at(&root, "up", &["conf-owner"]).code, 0);
    said(&root, "conf-owner", "| owns ");
    let owner = child(&root, "conf-owner");
    // Rewritten, as the program it belongs to may: what uses it was started
    // from its being there alone.
    let conf = root.join(".tay/state/files/kept-conf/kept-conf");
    std::fs::write(&conf, "rewritten by its owner\n").unwrap();
    let again = machine.at(&root, "up", &[]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    left_running(&again, "conf-owner");
    assert_eq!(child(&root, "conf-owner"), owner);
}

#[test]
fn a_unit_that_runs_what_a_command_builds_is_started_again_only_when_the_file_differs() {
    let machine = Machine::new();
    let root = machine.stack("changes");
    assert_eq!(machine.at(&root, "up", &["program"]).code, 0);
    said(&root, "program", "| says first");
    let program = child(&root, "program");
    // The build runs again, as a command with no check does at every
    // `tay up`, and builds the same program: what runs it is left running.
    let same = machine.at(&root, "up", &[]);
    assert_eq!(same.code, 0, "{}{}", same.stdout, same.stderr);
    assert!(
        same.changes()
            .iter()
            .any(|line| line.starts_with("build ready")),
        "{}",
        same.stdout
    );
    left_running(&same, "program");
    assert_eq!(child(&root, "program"), program);
    let changed = machine.at(&root, "up", &["--set", "settings.says=second"]);
    assert_eq!(changed.code, 0, "{}{}", changed.stdout, changed.stderr);
    assert!(
        changed
            .changes()
            .contains(&again("program", "the file `build.outputs.program`")),
        "{}",
        changed.stdout
    );
    said(&root, "program", "| says second");
}

#[test]
fn a_unit_whose_secret_changed_is_started_again_by_the_secrets_name_alone() {
    let machine = Machine::new();
    let root = machine.stack("changes");
    assert_eq!(machine.at(&root, "up", &["guarded"]).code, 0);
    said(&root, "guarded", "| has a token");
    let secret = || {
        let text = std::fs::read_to_string(root.join(".tay/state/secrets/token")).unwrap();
        let kept: serde_json::Value = serde_json::from_str(&text).unwrap();
        kept["value"].as_str().unwrap().to_string()
    };
    let first = secret();
    // A token that is no longer kept is generated again, another, and the
    // unit given it is started again.
    std::fs::remove_file(root.join(".tay/state/secrets/token")).unwrap();
    let changed = machine.at(&root, "up", &[]);
    assert_eq!(changed.code, 0, "{}{}", changed.stdout, changed.stderr);
    assert!(
        changed
            .changes()
            .contains(&again("guarded", "`secret_env.TOKEN`")),
        "{}",
        changed.stdout
    );
    let second = secret();
    assert_ne!(second, first);
    let started =
        std::fs::read_to_string(root.join(".tay/state/units/guarded/started-from.json")).unwrap();
    let status = machine.at(&root, "status", &["--json"]);
    for shown in [&started, &changed.stdout, &changed.stderr, &status.stdout] {
        for value in [&first, &second] {
            assert!(
                !shown.contains(value.as_str()),
                "a secret is shown: {shown}"
            );
        }
    }
}
