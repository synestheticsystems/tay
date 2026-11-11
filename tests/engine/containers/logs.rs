//! A container's output in its unit's log. A line the log cannot take, as
//! when the disk is full, is lost, as a host process's is: the container
//! goes on running, is replaced and removed as any other, and the log says
//! how many lines it lost once it can take one again.

use std::fs;
use std::path::Path;
use std::time::Duration;

use super::{container, labelled, name};
use crate::docker::docker;
use crate::harness::{Machine, eventually};
use crate::refused_log::{Refusing, Said, lost_once_and_counted, nearly_full, said_lost};

/// `talker` at `root`, brought up with its log nearly full, then that log
/// made to refuse lines, and `talker` told to print: the refusal, and the
/// container's id.
fn talking_to_a_refusing_log(machine: &Machine, root: &Path) -> (Refusing, serde_json::Value) {
    let folder = nearly_full(root, "talker");
    let up = machine.at(root, "up", &["talker"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let started = container(machine, root, "talker").unwrap()["Id"].clone();
    let refusing = Refusing::of(&folder);
    fs::write(root.join(".tay/state/talker/talk"), "").unwrap();
    (refusing, started)
}

/// Waits until the container of `talker` has printed line `number`, while
/// it runs.
fn printed(machine: &Machine, root: &Path, number: u64) {
    let line = format!("line {number}\n");
    let name = name(machine, root, "talker");
    let mut running = true;
    eventually(Duration::from_secs(120), || {
        running = container(machine, root, "talker")
            .is_some_and(|found| found["State"]["Running"] == true);
        !running || docker(&["logs", &name]).contains(&line)
    });
    let status = machine.at(root, "status", &[]).stdout;
    assert!(
        running && docker(&["logs", &name]).contains(&line),
        "`talker` did not print line {number} while it ran:\n{status}"
    );
}

#[test]
fn a_container_goes_on_while_its_log_cannot_be_written_and_the_log_then_says_how_many_lines_it_lost()
 {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let (refusing, started) = talking_to_a_refusing_log(&machine, &root);
    // Twenty seconds of lines, most of them refused: longer than watching a
    // container that fails is tried again before it is stopped.
    printed(&machine, &root, 100);
    let state = Machine::state(&machine.at(&root, "status", &["--json"]).json(), "talker");
    assert!(state.starts_with("ready attempt 1, running"), "{state}");
    drop(refusing);
    lost_once_and_counted(&said_lost(&root, "talker"));
    let found = container(&machine, &root, "talker").unwrap();
    assert_eq!(found["Id"], started);
    assert_eq!(found["State"]["Running"], true);
}

#[test]
fn a_container_whose_log_cannot_be_written_is_replaced_and_the_one_it_replaces_removed() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let (refusing, started) = talking_to_a_refusing_log(&machine, &root);
    // Lines past the length at which the log is set aside.
    printed(&machine, &root, 40);
    let restart = machine.at(&root, "restart", &["talker"]);
    assert_eq!(restart.code, 0, "{}{}", restart.stdout, restart.stderr);
    let replaced = container(&machine, &root, "talker").unwrap();
    assert_ne!(replaced["Id"], started);
    assert_eq!(replaced["State"]["Running"], true);
    assert_eq!(
        labelled(&machine, &root)[0],
        [name(&machine, &root, "talker")]
    );
    drop(refusing);
    let held = said_lost(&root, "talker");
    let notes = held.iter().filter(|said| matches!(said, Said::Lost(_)));
    assert_eq!(notes.count(), 1, "{held:?}");
}
