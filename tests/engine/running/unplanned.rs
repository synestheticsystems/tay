//! Units the plan no longer has: a unit that is running and is not in the
//! plan any more is stopped at the next `tay up`, which says so, and
//! `tay status` shows it until then.

use std::path::Path;
use std::time::Duration;

use crate::harness::{Machine, alive, child, eventually, log};

/// The definitions of the `leaving` stack written again without the unit
/// `leaving`.
fn without_leaving(root: &Path) {
    let definitions = std::fs::read_to_string(root.join("tay.yaml")).unwrap();
    let (before, after) = definitions.split_once("  leaving:\n").unwrap();
    let (_, rest) = after.split_once("  joining:\n").unwrap();
    let written =
        format!("{before}  joining:\n{rest}").replace("up: [staying, leaving]", "up: [staying]");
    std::fs::write(root.join("tay.yaml"), written).unwrap();
}

#[test]
fn a_unit_the_definitions_no_longer_have_is_shown_by_status_until_up_stops_it_and_says_so() {
    let machine = Machine::new();
    let root = machine.stack("leaving");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let leaving = child(&root, "leaving");
    machine.kill_daemon(&root);
    without_leaving(&root);
    // A daemon started afresh plans without it, and finds it running.
    let status = machine.at(&root, "status", &["--json"]);
    assert_eq!(status.code, 1, "{}", status.stdout);
    assert_eq!(
        Machine::state(&status.json(), "leaving"),
        "failed it runs, and is not in the plan any more: the next `tay up` stops it"
    );
    assert!(alive(leaving), "status stops nothing");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(
        up.last_change("leaving"),
        "stopped it is not in the plan any more, so it was stopped"
    );
    assert!(!alive(leaving));
    let status = machine.at(&root, "status", &["--json"]).json();
    let units: Vec<&str> = status["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|unit| unit["unit"].as_str().unwrap())
        .collect();
    assert_eq!(units, ["staying"], "{status}");
}

#[test]
fn units_left_out_of_what_up_is_asked_for_are_stopped_by_that_up() {
    let machine = Machine::new();
    let root = machine.stack("leaving");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let before = [child(&root, "staying"), child(&root, "leaving")];
    // The environment is asked for `joining` alone from now.
    let up = machine.at(&root, "up", &["joining"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    for unit in ["staying", "leaving"] {
        assert_eq!(
            up.last_change(unit),
            "stopped it is not in the plan any more, so it was stopped"
        );
    }
    assert!(before.iter().all(|pid| !alive(*pid)));
    assert!(alive(child(&root, "joining")));
}

#[test]
fn a_unit_found_running_that_comes_back_into_the_plan_is_adopted_and_not_started_again() {
    let machine = Machine::new();
    let root = machine.stack("leaving");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let leaving = child(&root, "leaving");
    let definitions = std::fs::read_to_string(root.join("tay.yaml")).unwrap();
    machine.kill_daemon(&root);
    without_leaving(&root);
    assert_eq!(machine.at(&root, "status", &[]).code, 1);
    std::fs::write(root.join("tay.yaml"), definitions).unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(child(&root, "leaving"), leaving);
    assert!(alive(leaving));
    machine.until_state(&root, "leaving", Duration::from_secs(20), |state| {
        state == "ready attempt 1, running"
    });
    assert_eq!(log(&root, "leaving").matches("starting").count(), 1);
}

#[test]
fn down_stops_a_unit_the_plan_does_not_have() {
    let machine = Machine::new();
    let root = machine.stack("leaving");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let leaving = child(&root, "leaving");
    machine.kill_daemon(&root);
    without_leaving(&root);
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    assert!(eventually(Duration::from_secs(10), || !alive(leaving)));
    assert!(machine.processes().is_empty(), "{:?}", machine.processes());
}
