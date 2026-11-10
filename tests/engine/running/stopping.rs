//! Stopping: a unit's whole group is signalled, given time, and then what
//! is left is killed, so that what a unit started in turn goes with it; and
//! `tay down` says nothing is left only once it has checked.

use std::time::{Duration, Instant};

use crate::harness::{Machine, child};

#[test]
fn stopping_a_unit_takes_the_processes_it_started_with_it() {
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &["spawner"]).code, 0);
    let group = child(&root, "spawner");
    let in_group = || {
        crate::harness::processes()
            .into_iter()
            .filter(|(_, other, _)| *other == group)
            .count()
    };
    assert!(
        crate::harness::eventually(Duration::from_secs(20), || in_group() >= 3),
        "the spawner and the two it started run"
    );
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}", down.stdout);
    assert_eq!(in_group(), 0);
}

#[test]
fn after_down_no_process_of_the_environment_exists_and_down_says_so_after_checking() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let up = machine.at(&root, "up", &["web", "spawner", "stubborn"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(!machine.processes().is_empty());
    let started = Instant::now();
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    assert!(
        down.stdout
            .ends_with("tay down: nothing is left running.\n"),
        "{}",
        down.stdout
    );
    // `stubborn` ignores the request to end, so it is killed once its time
    // to end has passed.
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert!(machine.processes().is_empty(), "{:?}", machine.processes());
    let log = crate::harness::log(&root, "stubborn");
    assert!(
        log.contains("# attempt 1, command: was killed by signal 9 (KILL)"),
        "{log}"
    );
}

#[test]
fn when_the_environments_folder_disappears_the_daemon_stops_everything_it_started_and_exits() {
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &["web", "spawner"]).code, 0);
    let daemon = machine.daemon_pid(&root).unwrap();
    // The records go with the folder; the daemon stops what it saw run.
    std::fs::remove_dir_all(root.join(".tay")).unwrap();
    assert!(
        crate::harness::eventually(Duration::from_secs(15), || {
            machine.processes().is_empty()
        }),
        "{:?}",
        machine.processes()
    );
    assert!(!crate::harness::alive(daemon));
}
