//! A daemon started while the definitions have errors cannot plan. It
//! still finds what runs of the environment, and looks after none of it
//! until the definitions plan again: nothing that ends is started again,
//! and the next `tay up` that plans takes everything up from there. Until
//! then `tay status` shows the errors with each unit found running and
//! exits with code 2, `tay restart` and `tay wipe` of a unit are refused
//! with the same errors, and `tay down` stops everything it finds.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::harness::{Machine, alive, child, eventually, record};

/// A unit a daemon with no plan found running, as `tay status` shows it.
const FOUND: &str = "failed something of it is left, and the daemon has no plan, so nothing looks after it until `tay up` plans";

/// What `tay COMMAND` closes the errors with while the daemon has no plan.
fn closing(command: &str) -> String {
    format!(
        "tay {command}: 1 error in the definitions; the daemon has no plan, so it looks after nothing until `tay up` plans.\n"
    )
}

/// The stack `leaving`, brought up, whose daemon is then killed and whose
/// definitions are given an error, as a change of branch might give them:
/// the next daemon cannot plan. Gives its root and its definitions as they
/// were.
fn found_without_plan(machine: &Machine) -> (PathBuf, String) {
    let root = machine.stack("leaving");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    machine.kill_daemon(&root);
    let stack = fs::read_to_string(root.join("tay.yaml")).unwrap();
    let broken = stack.replace("up: [staying, leaving]", "up: [staying, leaving, nobody]");
    fs::write(root.join("tay.yaml"), broken).unwrap();
    (root, stack)
}

/// The errors `tay plan` prints for the stack at `root`, each on its line,
/// without its closing line.
fn as_plan_prints_them(machine: &Machine, root: &Path) -> Vec<String> {
    let plan = machine.at(root, "plan", &[]);
    assert_eq!(plan.code, 2, "{}{}", plan.stdout, plan.stderr);
    plan.errors().iter().map(|line| line.to_string()).collect()
}

/// The units a status lists, each as `unit state detail`, in the order of
/// their names.
fn listed(changes: Vec<String>) -> Vec<String> {
    let mut changes = changes;
    changes.sort();
    changes
}

#[test]
fn status_shows_the_errors_with_each_unit_found_running_and_restart_and_wipe_are_refused_with_them()
{
    let machine = Machine::new();
    let (root, _) = found_without_plan(&machine);
    let errors = as_plan_prints_them(&machine, &root);
    let printed: String = errors.iter().map(|line| format!("{line}\n")).collect();
    let status = machine.at(&root, "status", &[]);
    assert_eq!(status.code, 2, "{}{}", status.stdout, status.stderr);
    assert_eq!(
        listed(status.changes()),
        [format!("leaving {FOUND}"), format!("staying {FOUND}")]
    );
    assert_eq!(status.stderr, format!("{printed}{}", closing("status")));
    let json = machine.at(&root, "status", &["--json"]);
    assert_eq!(json.code, 2, "{}{}", json.stdout, json.stderr);
    let json = json.json();
    assert_eq!(Machine::state(&json, "staying"), FOUND);
    assert_eq!(Machine::state(&json, "leaving"), FOUND);
    assert_eq!(
        json["no_plan"],
        json!({ "too_old": false, "errors": errors })
    );
    let restart = machine.at(&root, "restart", &["staying"]);
    assert_eq!(
        (restart.code, restart.stderr),
        (2, format!("{printed}{}", closing("restart")))
    );
    let wipe = machine.at(&root, "wipe", &["staying", "--yes"]);
    assert_eq!(
        (wipe.code, wipe.stderr),
        (
            2,
            format!("{printed}tay wipe: 1 error in the definitions; nothing was removed.\n")
        )
    );
}

#[test]
fn nothing_that_ends_is_started_again_and_the_next_up_that_plans_takes_everything_up_from_there() {
    let machine = Machine::new();
    let (root, stack) = found_without_plan(&machine);
    let staying = child(&root, "staying");
    let leaving = child(&root, "leaving");
    assert_eq!(machine.at(&root, "status", &[]).code, 2);
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(leaving).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    // Nothing starts it again: once nothing of it is left, it is not
    // listed, and its record says that its first attempt ended.
    let gone = eventually(Duration::from_secs(20), || {
        let status = machine.at(&root, "status", &["--json"]).json();
        status["units"].as_array().unwrap().len() == 1
    });
    assert!(gone, "{}", machine.at(&root, "status", &[]).stdout);
    let ended = record(&root, "leaving");
    assert_eq!(ended["attempt"], 1, "{ended}");
    assert!(!ended["ended"].is_null(), "{ended}");
    // The definitions plan again, and the daemon that could not plan is
    // the one `tay up` asks.
    let daemon = machine.daemon_pid(&root);
    fs::write(root.join("tay.yaml"), stack).unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(machine.daemon_pid(&root), daemon);
    let status = machine.settled(&root);
    assert!(Machine::state(&status, "staying").starts_with("ready "));
    assert!(Machine::state(&status, "leaving").starts_with("ready "));
    // What ran was looked after again as it ran, not started again.
    assert_eq!(child(&root, "staying"), staying);
    assert!(alive(staying));
    assert_ne!(child(&root, "leaving"), leaving);
}

#[test]
fn down_stops_everything_a_daemon_with_no_plan_finds() {
    let machine = Machine::new();
    let (root, _) = found_without_plan(&machine);
    let programs = [child(&root, "staying"), child(&root, "leaving")];
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    assert_eq!(
        listed(down.changes()),
        [
            "leaving stopped stopped by `tay down`",
            "staying stopped stopped by `tay down`"
        ]
    );
    assert!(
        down.stdout
            .ends_with("tay down: nothing is left running.\n"),
        "{}",
        down.stdout
    );
    for program in programs {
        assert!(!alive(program));
    }
    assert!(machine.daemon_pid(&root).is_none());
    assert_eq!(
        fs::read_to_string(root.join(".tay/state/intent")).unwrap(),
        "down\n"
    );
}

#[test]
fn a_remembered_choice_that_no_longer_fits_is_shown_as_remembered_with_what_forgets_it() {
    let machine = Machine::new();
    let root = machine.stack("leaving");
    let up = machine.at(&root, "up", &["joining"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    machine.kill_daemon(&root);
    // Another branch has no unit `joining`, which the environment
    // remembers being asked for.
    let stack = fs::read_to_string(root.join("tay.yaml")).unwrap();
    let (kept, _) = stack.split_once("  joining:").unwrap();
    fs::write(root.join("tay.yaml"), kept).unwrap();
    let status = machine.at(&root, "status", &[]);
    assert_eq!(status.code, 2, "{}{}", status.stdout, status.stderr);
    assert_eq!(status.changes()[1..], [format!("joining {FOUND}")]);
    assert_eq!(
        status.stderr,
        format!(
            "remembered `tay up joining`: there is no unit `joining`; `tay unset units` forgets the units named then\n{}",
            closing("status")
        )
    );
    let unset = machine.at(&root, "unset", &["units"]);
    assert_eq!(unset.code, 0, "{}{}", unset.stdout, unset.stderr);
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(
        up.changes().contains(
            &"joining stopped it is not in the plan any more, so it was stopped".to_string()
        ),
        "{}",
        up.stdout
    );
    let status = machine.settled(&root);
    assert!(Machine::state(&status, "staying").starts_with("ready "));
}
