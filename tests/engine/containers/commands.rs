//! A command in a container: with `in`, inside the named unit's running
//! container, as `user` if given; with `image`, in a container of its own
//! with `run` in place of the image's entrypoint, removed when it ends.
//! Either way its `check` runs first, its secrets reach it through its
//! environment, and the values its `TAY_VALUES` file holds are fetched from
//! where it ran.

use std::time::Duration;

use super::labelled;
use crate::harness::{Machine, eventually, log};

#[test]
fn commands_in_a_container_check_then_run_hand_on_their_values_and_leave_no_container() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["shown"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(
        up.last_change("seeded"),
        "ready done: it ran and exited with 0"
    );
    assert_eq!(
        up.last_change("stamped"),
        "ready done: it ran and exited with 0"
    );
    assert!(eventually(Duration::from_secs(20), || {
        log(&root, "shown").contains("| seed=a handful of seeds stamp=stamped")
    }));
    // Each checked, found itself not done, and ran, fed what `stdin` names.
    let seeded = log(&root, "seeded");
    let steps: Vec<&str> = seeded
        .lines()
        .filter_map(|line| line.split_once(" # ").map(|(_, note)| note))
        .collect();
    assert_eq!(steps.len(), 4, "{seeded}");
    assert_eq!(
        steps[0],
        "attempt 1, check: starting `/services/seeded.sh` in the container of `store`"
    );
    assert!(
        steps[1].starts_with("attempt 1, check: exited with 1 at "),
        "{seeded}"
    );
    assert_eq!(
        steps[2],
        "attempt 1, run: starting `/services/seed.sh` in the container of `store`"
    );
    assert!(
        steps[3].starts_with("attempt 1, run: exited with 0 at "),
        "{seeded}"
    );
    assert!(
        seeded.contains("| seeded with a handful of seeds"),
        "{seeded}"
    );
    let stamped = log(&root, "stamped");
    let check = stamped.find("| not stamped yet").unwrap();
    let run = stamped.find("| stamping a handful of seeds").unwrap();
    assert!(check < run, "{stamped}");
    // Its secret reached it through its environment, and what it gave of
    // it is kept as a secret.
    let kept = std::fs::read_to_string(root.join(".tay/state/secrets/stamped")).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&kept).unwrap()["proof"],
        "32"
    );
    // The files they wrote their values to are not left in the store.
    let store = super::name(&machine, &root, "store");
    let kept = crate::docker::docker(&["exec", &store, "ls", "/tmp"]);
    assert!(
        kept.contains("seed") && !kept.contains("tay-values"),
        "{kept}"
    );
    // Nothing is left of the container the stamp was made in; `store` is
    // the only container.
    let [containers, _, _] = labelled(&machine, &root);
    assert_eq!(containers, [super::name(&machine, &root, "store")]);
    // The next `tay up` finds the store seeded by its check, which gives
    // the seed again.
    let again = machine.at(&root, "up", &["shown"]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    let status = machine.at(&root, "status", &["--json"]).json();
    assert_eq!(
        Machine::state(&status, "seeded"),
        "ready done: its check exited with 0"
    );
}

#[test]
fn a_commands_container_found_created_and_never_started_is_not_done() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    assert_eq!(machine.at(&root, "up", &["stamped"]).code, 0);
    // As a daemon leaves it that is killed between making the container of
    // a step and starting it.
    machine.kill_daemon(&root);
    let id = machine.id(&root);
    let name = super::name(&machine, &root, "stamped");
    crate::docker::docker(&[
        "create",
        "--name",
        &name,
        "--memory",
        "16m",
        "--label",
        &format!("tay.environment={id}"),
        "--label",
        "tay.unit=stamped",
        "--label",
        "tay.attempt=1",
        "--label",
        "tay.step=run",
        "busybox:1.36",
        "true",
    ]);
    let status = machine.at(&root, "status", &["--json"]).json();
    assert_eq!(
        Machine::state(&status, "stamped"),
        "stopped it has not been started here"
    );
    assert!(super::container(&machine, &root, "stamped").is_none());
}
