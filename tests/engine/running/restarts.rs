//! Restarts: when a unit that runs ends, its `restart` decides; `attempts`
//! counts attempts in a row that did not reach ready, and reaching ready
//! starts the count again. `tay up` succeeds only when everything asked for
//! is ready, and a unit that failed an attempt and then became ready counts
//! as ready.

use std::time::Duration;

use crate::harness::{Machine, child};

#[test]
fn up_exits_0_only_when_everything_asked_for_is_ready() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let broken = machine.at(&root, "up", &["broken"]);
    assert_eq!(broken.code, 1, "{}{}", broken.stdout, broken.stderr);
    let lines: Vec<&str> = broken.stdout.lines().collect();
    let failed = lines
        .iter()
        .position(|line| line.starts_with("tay up: `broken` failed: attempt 2 exited with 3 at "))
        .unwrap_or_else(|| panic!("{}", broken.stdout));
    assert!(
        lines[failed].ends_with("; 2 attempts in a row did not become ready"),
        "{}",
        lines[failed]
    );
    // With the last lines it wrote, each attempt marked.
    assert_eq!(lines[failed + 1], "  the last lines it wrote:");
    let shown = lines[failed + 2..].join("\n");
    assert!(shown.contains("| no configuration was found"), "{shown}");
    assert!(
        shown.contains("# attempt 2, command: exited with 3"),
        "{shown}"
    );
    let never = machine.at(&root, "up", &["never-ready"]);
    assert_eq!(never.code, 1, "{}{}", never.stdout, never.stderr);
    assert!(
        never.stdout.contains(
            "tay up: `never-ready` failed: attempt 2 did not become ready within 2s, and was stopped: no line it wrote has it yet; 2 attempts in a row did not become ready"
        ),
        "{}",
        never.stdout
    );
    let web = machine.at(&root, "up", &["web"]);
    assert_eq!(web.code, 0, "{}{}", web.stdout, web.stderr);
    let status = machine.at(&root, "status", &["--json"]).json();
    for unit in ["api", "web"] {
        assert!(
            Machine::state(&status, unit).starts_with("ready "),
            "{status}"
        );
    }
}

#[test]
fn a_service_slow_to_be_ready_makes_what_needs_it_wait_and_then_start_in_one_up() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let started = std::time::Instant::now();
    let up = machine.at(&root, "up", &["web"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(started.elapsed() >= Duration::from_secs(2));
    let changes = up.changes();
    let at = |line: &str| {
        changes
            .iter()
            .position(|change| change == line)
            .unwrap_or_else(|| panic!("`{line}` in {changes:?}"))
    };
    assert!(at("web waiting for `api`") < at("api ready attempt 1, log \"listening\" passes"));
    assert!(
        at("api ready attempt 1, log \"listening\" passes")
            < at("web starting attempt 1, starting")
    );
    assert_eq!(up.last_change("web"), "ready attempt 1, running");
    assert!(
        up.stdout
            .ends_with("tay up: everything asked for is ready.\n")
    );
}

#[test]
fn a_service_killed_shows_in_status_within_ten_seconds_with_how_it_ended() {
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &["web"]).code, 0);
    let web = child(&root, "web");
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(web).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    let state = machine.until_state(&root, "web", Duration::from_secs(10), |state| {
        state.contains("attempt 1 was killed by signal 9 (KILL) at ")
    });
    // It restarts on failure, as a process does unless it says otherwise.
    assert!(state.contains("attempt 2,"), "{state}");
    machine.until_state(&root, "web", Duration::from_secs(20), |state| {
        state.starts_with("ready attempt 2, running; attempt 1 was killed by signal 9 (KILL) at ")
    });
}

#[test]
fn reaching_ready_starts_the_count_of_attempts_again() {
    // `quitter` is ready once it has started, and exits with 7 two seconds
    // later; it may fail three attempts in a row, and never does.
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &["quitter"]).code, 0);
    machine.until_state(&root, "quitter", Duration::from_secs(40), |state| {
        state.starts_with("ready attempt 4, running; attempt 3 exited with 7 at ")
    });
}
