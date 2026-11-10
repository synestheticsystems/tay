//! Running an action that uses a container: the container is brought up
//! for it with the port the action uses from the host, and stopped again;
//! one that is up without that port is never replaced for it, and the run
//! is refused, naming it.

use std::time::Duration;

use super::{container, labelled};
use crate::harness::{Machine, eventually};

#[test]
fn an_action_that_would_replace_a_container_that_is_up_is_refused_naming_it() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["quiet"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let before = container(&machine, &root, "quiet").unwrap();
    let run = machine.at(&root, "run", &["ask-quiet"]);
    assert_eq!(run.code, 4, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.stderr,
        "tay run: running `ask-quiet` would replace `quiet`, which is up, to open its port 9090 on the host; `tay run` never stops, restarts or replaces a unit that is up\n"
    );
    // The container runs as it did, its port not opened, and nothing ran.
    let after = container(&machine, &root, "quiet").unwrap();
    assert_eq!(after["Id"], before["Id"]);
    assert_eq!(after["State"]["Running"], true);
    assert!(!root.join(".tay/logs/ask-quiet").exists());
}

#[test]
fn an_action_brings_up_a_container_it_needs_with_the_port_it_uses_and_stops_it_again() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let run = machine.at(&root, "run", &["ask-quiet"]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let port = crate::harness::kept_ports(&root)["quiet-port"].clone();
    assert_eq!(run.stdout, format!("asked 127.0.0.1:{port}\n"));
    // Nothing of the environment is left in Docker, and the daemon
    // started for the run leaves.
    assert!(
        eventually(Duration::from_secs(20), || machine.processes().is_empty()),
        "{:?}",
        machine.processes()
    );
    let [containers, _, networks] = labelled(&machine, &root);
    assert!(containers.is_empty(), "{containers:?}");
    assert!(networks.is_empty(), "{networks:?}");
}
