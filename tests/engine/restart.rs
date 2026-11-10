//! `tay restart UNIT...`: each unit named is stopped and started again with
//! a new first attempt, what it needs brought up with it; units that need
//! it are left running; a command is run again from its check; and a unit
//! that runs nothing has nothing to restart.

use std::fs;

use crate::harness::{Machine, child, log};

#[test]
fn restart_stops_a_process_and_starts_it_again_and_leaves_what_needs_it_running() {
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &["web"]).code, 0);
    let (api, web) = (child(&root, "api"), child(&root, "web"));
    let restart = machine.at(&root, "restart", &["api"]);
    assert_eq!(restart.code, 0, "{}{}", restart.stdout, restart.stderr);
    assert_eq!(
        restart.changes()[..2],
        [
            "api stopped stopped by `tay restart`",
            "api starting attempt 1, starting"
        ]
    );
    assert!(
        restart
            .stdout
            .ends_with("tay restart: everything asked for is ready.\n"),
        "{}",
        restart.stdout
    );
    assert_ne!(child(&root, "api"), api);
    // The web, which needs it, is left as it runs.
    assert_eq!(child(&root, "web"), web);
    assert!(!restart.stdout.contains("  web "), "{}", restart.stdout);
    // Each start is a first attempt, and the log marks both.
    assert_eq!(
        log(&root, "api")
            .matches("# attempt 1, command: starting")
            .count(),
        2
    );
    // Restarting the web brings up what it needs, the API, which runs and
    // is joined as it is.
    let restart = machine.at(&root, "restart", &["web"]);
    assert_eq!(restart.code, 0, "{}{}", restart.stdout, restart.stderr);
    assert_ne!(child(&root, "web"), web);
    let api = child(&root, "api");
    assert!(
        log(&root, "api")
            .matches("# attempt 1, command: starting")
            .count()
            == 2
            && child(&root, "api") == api
    );
}

#[test]
fn restart_runs_a_command_again_from_its_check() {
    let machine = Machine::new();
    let root = machine.stack("commands");
    assert_eq!(machine.at(&root, "up", &["checked"]).code, 0);
    let done = root.join(".tay/state/checked-done");
    assert_eq!(fs::read_to_string(&done).unwrap(), "run\n");
    // Its check finds it done, and it does not run.
    let restart = machine.at(&root, "restart", &["checked"]);
    assert_eq!(restart.code, 0, "{}{}", restart.stdout, restart.stderr);
    assert_eq!(
        restart.last_change("checked"),
        "ready done: its check exited with 0"
    );
    assert_eq!(fs::read_to_string(&done).unwrap(), "run\n");
    // Its work gone, its check finds it not done, and it runs.
    fs::remove_file(&done).unwrap();
    let restart = machine.at(&root, "restart", &["checked"]);
    assert_eq!(
        restart.last_change("checked"),
        "ready done: it ran and exited with 0"
    );
    assert_eq!(fs::read_to_string(&done).unwrap(), "run\n");
}

#[test]
fn restart_of_a_unit_that_runs_nothing_or_is_not_planned_or_of_an_environment_down_is_an_error() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let never = machine.at(&root, "restart", &["web"]);
    assert_eq!(
        (never.code, never.stderr.as_str()),
        (
            2,
            "tay restart: this environment has never been brought up: `tay up` brings it up\n"
        )
    );
    assert_eq!(machine.at(&root, "up", &["ticking"]).code, 0);
    let made = machine.at(&root, "restart", &["ticks-port"]);
    assert_eq!(
        (made.code, made.stderr.as_str()),
        (
            2,
            "tay restart: `ticks-port` runs nothing, so it has nothing to restart: only a process, a container or a command does\n"
        )
    );
    let unknown = machine.at(&root, "restart", &["nowhere"]);
    assert_eq!(
        (unknown.code, unknown.stderr.as_str()),
        (
            2,
            "tay restart: there is no unit `nowhere` in this environment's plan\n"
        )
    );
    let ticking = child(&root, "ticking");
    assert_eq!(machine.at(&root, "down", &[]).code, 0);
    let down = machine.at(&root, "restart", &["ticking"]);
    assert_eq!(
        (down.code, down.stderr.as_str()),
        (
            2,
            "tay restart: the environment is down: `tay up` brings it up\n"
        )
    );
    assert!(!crate::harness::alive(ticking));
}

#[test]
fn restart_replaces_a_container_with_one_on_its_first_attempt() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    assert_eq!(machine.at(&root, "up", &["ticker"]).code, 0);
    let first = crate::containers::container(&machine, &root, "ticker").unwrap();
    let restart = machine.at(&root, "restart", &["ticker"]);
    assert_eq!(restart.code, 0, "{}{}", restart.stdout, restart.stderr);
    let again = crate::containers::container(&machine, &root, "ticker").unwrap();
    assert_ne!(again["Id"], first["Id"]);
    assert_eq!(again["Config"]["Labels"]["tay.attempt"], "1");
    assert_eq!(again["State"]["Running"], true);
}
