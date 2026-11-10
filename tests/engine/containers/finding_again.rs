//! Finding containers again: a new daemon asks Docker for the containers
//! with the environment's label and adopts one that runs, without starting
//! it again, copying what it wrote while no daemon ran into its log with
//! Docker's times. And at `tay up`, a container whose settings have changed
//! is replaced, its volumes kept, while one whose settings have not is left
//! running, its image judged by what it is, not by the name it is given.

use std::time::Duration;

use super::container;
use crate::harness::{Machine, eventually, log};

/// The numbers of the ticks in the log of `ticker` at `root`, in the order
/// the log holds them.
fn ticks(root: &std::path::Path) -> Vec<u64> {
    log(root, "ticker")
        .lines()
        .filter_map(|line| line.split_once(" | tick "))
        .map(|(_, number)| number.parse().unwrap())
        .collect()
}

#[test]
fn a_new_daemon_adopts_a_running_container_without_starting_it_again() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    assert_eq!(machine.at(&root, "up", &["ticker"]).code, 0);
    let started = container(&machine, &root, "ticker").unwrap()["Id"].clone();
    machine.kill_daemon(&root);
    machine.until_state(&root, "ticker", Duration::from_secs(20), |state| {
        state == "ready attempt 1, running; ready when it was found again"
    });
    let found = container(&machine, &root, "ticker").unwrap();
    assert_eq!(found["Id"], started);
    assert_eq!(found["State"]["Running"], true);
    assert_eq!(log(&root, "ticker").matches(" # attempt ").count(), 1);
}

#[test]
fn what_a_container_wrote_while_no_daemon_ran_is_in_its_log_with_its_times_once_one_runs() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    assert_eq!(machine.at(&root, "up", &["ticker"]).code, 0);
    assert!(eventually(Duration::from_secs(20), || ticks(&root).len() >= 2));
    machine.kill_daemon(&root);
    let at_kill = *ticks(&root).last().unwrap();
    std::thread::sleep(Duration::from_secs(3));
    // No daemon copies what it writes meanwhile; Docker holds it.
    assert!(
        ticks(&root).last().unwrap() - at_kill <= 1,
        "{:?}",
        ticks(&root)
    );
    let status = machine.at(&root, "status", &[]);
    assert_eq!(status.code, 0, "{}{}", status.stdout, status.stderr);
    assert!(eventually(Duration::from_secs(20), || {
        ticks(&root).last().is_some_and(|last| *last >= at_kill + 4)
    }));
    // Each line once, in order, with none missing.
    let ticks = ticks(&root);
    assert!(
        ticks.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "{ticks:?}"
    );
    // Each with the time Docker kept for it: a second apart, as written,
    // not the moment it was copied.
    let times: Vec<String> = log(&root, "ticker")
        .lines()
        .filter(|line| line.contains(" | tick "))
        .map(|line| line[..24].to_string())
        .collect();
    let seconds = |time: &str| -> f64 {
        let (hours, rest) = time[11..23].split_at(2);
        let minutes: f64 = rest[1..3].parse().unwrap();
        let seconds: f64 = rest[4..].parse().unwrap();
        hours.parse::<f64>().unwrap() * 3600.0 + minutes * 60.0 + seconds
    };
    for pair in times.windows(2) {
        let gap = seconds(&pair[1]) - seconds(&pair[0]);
        assert!((0.5..1.5).contains(&gap), "{pair:?}");
    }
}

#[test]
fn a_container_whose_settings_changed_is_replaced_keeping_its_volume_and_one_unchanged_left_running()
 {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    assert_eq!(machine.at(&root, "up", &["keeper-data"]).code, 0);
    assert!(eventually(Duration::from_secs(20), || {
        log(&root, "keeper-data").contains("| mode one, start 1")
    }));
    let first = container(&machine, &root, "keeper-data").unwrap()["Id"].clone();
    let again = machine.at(&root, "up", &["keeper-data"]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert_eq!(
        container(&machine, &root, "keeper-data").unwrap()["Id"],
        first
    );
    assert!(again.changes().is_empty(), "{}", again.stdout);
    let changed = machine.at(&root, "up", &["keeper-data", "--set", "settings.mode=two"]);
    assert_eq!(changed.code, 0, "{}{}", changed.stdout, changed.stderr);
    assert_eq!(
        changed.changes()[0],
        "keeper-data starting `env.MODE` changed, so it is started again"
    );
    let replaced = container(&machine, &root, "keeper-data").unwrap();
    assert_ne!(replaced["Id"], first);
    assert_eq!(replaced["Config"]["Labels"]["tay.attempt"], "1");
    // The volume is the one it had: it counts both starts.
    assert!(eventually(Duration::from_secs(20), || {
        log(&root, "keeper-data").contains("| mode two, start 2")
    }));
}

#[test]
fn a_container_is_compared_by_the_image_it_was_created_from_not_the_name_that_names_it() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    assert_eq!(machine.at(&root, "up", &["tagged"]).code, 0);
    let first = container(&machine, &root, "tagged").unwrap()["Id"].clone();
    // The same image, named in full.
    let renamed = machine.at(
        &root,
        "up",
        &["--set", "settings.box=docker.io/library/busybox:1.36"],
    );
    assert_eq!(renamed.code, 0, "{}{}", renamed.stdout, renamed.stderr);
    assert!(renamed.changes().is_empty(), "{}", renamed.stdout);
    assert_eq!(container(&machine, &root, "tagged").unwrap()["Id"], first);
}

#[test]
fn what_a_container_wrote_is_copied_once_what_it_says_as_it_stops_included() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    assert_eq!(machine.at(&root, "up", &["farewell"]).code, 0);
    assert!(eventually(Duration::from_secs(20), || {
        log(&root, "farewell").contains("| three")
    }));
    // It goes quiet; then it is stopped.
    std::thread::sleep(Duration::from_secs(1));
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    let log = log(&root, "farewell");
    for said in ["| one", "| two", "| three", "| farewell"] {
        assert_eq!(log.matches(said).count(), 1, "{said}: {log}");
    }
}
