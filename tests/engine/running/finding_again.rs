//! Finding units again: a new daemon reads each unit's record and tries its
//! lock. Held: running under a live keeper, adopted (in `keeper.rs`). Free
//! with an ending: ended while no daemon ran, its exit code and time known,
//! and its restart policy applies. Free with no ending: gone with no
//! record, reported as exactly that, and what is left of its group stopped.

use std::time::Duration;

use crate::harness::{Machine, alive, child, eventually, record};

#[test]
fn a_unit_that_exits_while_no_daemon_runs_is_reported_with_its_exit_code_and_time_and_its_policy_applies()
 {
    let machine = Machine::new();
    let root = machine.stack("services");
    let up = machine.at(&root, "up", &["quitter", "once"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    machine.kill_daemon(&root);
    // Each exits with 7 two seconds after it started, with no daemon.
    assert!(eventually(Duration::from_secs(20), || {
        ["quitter", "once"]
            .iter()
            .all(|unit| !record(&root, unit)["ended"].is_null())
    }));
    let ended = record(&root, "once")["ended"]["at"].as_u64().unwrap() / 1000;
    let status = machine.at(&root, "status", &["--json"]).json();
    let once = Machine::state(&status, "once");
    assert!(
        once.starts_with("failed attempt 1 exited with 7 at 20")
            && once.ends_with(
                ", while no daemon ran; it is not started again, since its `restart` is `never`"
            ),
        "{once}"
    );
    // The time is the one the keeper recorded, as a person reads it.
    let at = &once["failed attempt 1 exited with 7 at ".len()..][..19];
    let seconds = |text: &str| {
        let (date, time) = text.split_once(' ').unwrap();
        let [hours, minutes, secs]: [u64; 3] = time
            .split(':')
            .map(|part| part.parse().unwrap())
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        (date.to_string(), hours * 3600 + minutes * 60 + secs)
    };
    assert_eq!(seconds(at).1, ended % 86400, "{at}");
    // `quitter` restarts on failure: a new attempt, saying how the last ended.
    let quitter = machine.until_state(&root, "quitter", Duration::from_secs(20), |state| {
        state.contains("attempt 2, running")
    });
    assert!(
        quitter.contains("; attempt 1 exited with 7 at ")
            && quitter.contains(", while no daemon ran"),
        "{quitter}"
    );
}

#[test]
fn a_unit_whose_keeper_was_lost_is_reported_so_and_what_is_left_of_its_group_is_stopped() {
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &["spawner", "web"]).code, 0);
    let spawner = record(&root, "spawner");
    let web = child(&root, "web");
    machine.kill_daemon(&root);
    for unit in ["spawner", "web"] {
        let keeper = record(&root, unit)["keeper"]["pid"].as_i64().unwrap() as i32;
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(keeper).unwrap(),
            rustix::process::Signal::KILL,
        )
        .unwrap();
    }
    // A child that prints dies at its next line, its pipe gone; a silent one
    // lives on in its group.
    assert!(eventually(Duration::from_secs(20), || !alive(web)));
    let group = spawner["child"]["pid"].as_i64().unwrap() as i32;
    let in_group = || {
        crate::harness::processes()
            .into_iter()
            .filter(|(_, other, _)| *other == group)
            .count()
    };
    assert!(
        in_group() >= 3,
        "the spawner and the two it started live on"
    );
    for unit in ["spawner", "web"] {
        let state = machine.until_state(&root, unit, Duration::from_secs(15), |state| {
            state.starts_with("failed")
        });
        assert_eq!(
            state,
            "failed attempt 1 is gone, with no record of how it ended: its keeper was lost, and what was left of it was stopped"
        );
    }
    assert_eq!(in_group(), 0, "what was left of the group is stopped");
}

#[test]
fn the_attempts_in_a_row_are_counted_on_by_a_daemon_started_afresh() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let up = machine.spawn_up(&root, &["doomed"]);
    // Attempt 1 exits before it is ready; attempt 2 begins, and its daemon
    // is killed.
    let recorded = root.join(".tay/state/units/doomed/record.json");
    assert!(eventually(Duration::from_secs(20), || {
        recorded.exists() && record(&root, "doomed")["attempt"] == 2
    }));
    machine.kill_daemon(&root);
    up.finish();
    assert!(eventually(Duration::from_secs(20), || {
        !record(&root, "doomed")["ended"].is_null()
    }));
    let state = machine.until_state(&root, "doomed", Duration::from_secs(20), |state| {
        state.starts_with("failed")
    });
    assert!(
        state.starts_with("failed attempt 2 exited with 7 at ")
            && state.ends_with(", while no daemon ran; 2 attempts in a row did not become ready"),
        "{state}"
    );
}
