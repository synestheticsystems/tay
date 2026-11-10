//! How a container ended, read from Docker: its exit code, and whether it
//! was killed for memory, in its unit's detail and its log; then its
//! `restart` decides, its attempts counted as a process's are.

use std::time::Duration;

use super::name;
use crate::docker::{docker, inspect};
use crate::harness::{Machine, eventually, log};

#[test]
fn a_container_that_exits_during_start_is_restarted_saying_how_it_ended() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["quitter"]);
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    let restarted = up
        .changes()
        .into_iter()
        .find(|line| {
            line.starts_with("quitter starting attempt 2, to start; attempt 1 exited with 3 at 20")
        })
        .unwrap_or_else(|| panic!("the restart is said:\n{}", up.stdout));
    assert!(restarted.ends_with(" UTC"), "{restarted}");
    let failed = up.last_change("quitter");
    assert!(
        failed.starts_with("failed attempt 2 exited with 3 at 20")
            && failed.ends_with(" UTC; 2 attempts in a row did not become ready"),
        "{failed}"
    );
    assert!(!up.stdout.contains("memory"), "{}", up.stdout);
    let log = log(&root, "quitter");
    for note in [
        "# attempt 1, container: exited with 3",
        "# attempt 2, container: exited with 3",
    ] {
        assert!(log.contains(note), "{note}: {log}");
    }
    // The container of the first attempt was removed when the second began,
    // its last words copied first.
    assert_eq!(log.matches("| giving up").count(), 2, "{log}");
    // Each attempt ran in a container of its own; the last is left, ended,
    // until the environment is taken down.
    let last = inspect(&name(&machine, &root, "quitter")).unwrap();
    assert_eq!(last["Config"]["Labels"]["tay.attempt"], "2");
    assert_eq!(last["State"]["ExitCode"], 3);
}

#[test]
fn a_container_killed_for_memory_says_so() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["hungry"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let hungry = name(&machine, &root, "hungry");
    // Once the test's own limit is on the container, a smaller one is set,
    // and it is told to eat.
    assert!(eventually(Duration::from_secs(10), || {
        inspect(&hungry).is_some_and(|container| container["HostConfig"]["Memory"] == 64 << 20)
    }));
    docker(&["update", "--memory", "16m", "--memory-swap", "16m", &hungry]);
    std::fs::write(root.join(".tay/state/hungry/eat"), "").unwrap();
    let failed = machine.until_state(&root, "hungry", Duration::from_secs(30), |state| {
        state.starts_with("failed")
    });
    assert!(
        failed.starts_with("failed attempt 1 exited with 137 at 20")
            && failed.ends_with(
                " UTC, killed for memory; it is not started again, since its `restart` is `never`"
            ),
        "{failed}"
    );
    let log = log(&root, "hungry");
    assert!(
        log.contains("# attempt 1, container: exited with 137, killed for memory"),
        "{log}"
    );
    assert!(log.contains("| eating"), "{log}");
}
