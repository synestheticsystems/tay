//! A `command` on the host: its `check` first, if it has one (0 done, 1 not
//! done, anything else cannot tell), then its `run` with its `stdin`; the
//! values it writes to the file `TAY_VALUES` names, kept and given; and
//! every file under `outputs` there afterwards. A command killed part-way is
//! not done, and runs again at the next `tay up`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use crate::harness::{Machine, child, eventually, record};

fn runs(root: &std::path::Path, file: &str) -> usize {
    fs::read_to_string(root.join(".tay/state").join(file))
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

fn kill(pid: i32) {
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
}

#[test]
fn a_command_killed_part_way_is_not_done_and_runs_again_at_the_next_up() {
    let machine = Machine::new();
    let root = machine.stack("commands");
    let up = machine.spawn_up(&root, &["counted"]);
    assert!(eventually(Duration::from_secs(20), || runs(&root, "runs") == 1));
    kill(child(&root, "counted"));
    let up = up.finish();
    assert_eq!(up.code, 1, "{}", up.stdout);
    let failed = up.last_change("counted");
    assert!(
        failed.starts_with("failed attempt 1 ran and was killed by signal 9 (KILL) at ")
            && failed.ends_with("; it is not started again, since its `restart` is `never`"),
        "{failed}"
    );
    fs::write(root.join(".tay/state/go"), "").unwrap();
    let again = machine.at(&root, "up", &["counted"]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert_eq!(runs(&root, "runs"), 2);
    assert_eq!(
        again.last_change("counted"),
        "ready done: it ran and exited with 0"
    );
    // With no `check`, it runs at every `tay up`.
    assert_eq!(machine.at(&root, "up", &["counted"]).code, 0);
    assert_eq!(runs(&root, "runs"), 3);
}

#[test]
fn a_command_whose_daemon_was_killed_during_it_runs_again_at_the_next_up() {
    let machine = Machine::new();
    let root = machine.stack("commands");
    let up = machine.spawn_up(&root, &["counted"]);
    assert!(eventually(Duration::from_secs(20), || runs(&root, "runs") == 1));
    // The daemon, the keeper and the command all go, as when the machine
    // they ran on is put to sleep and woken without them.
    let record = record(&root, "counted");
    machine.kill_daemon(&root);
    kill(record["keeper"]["pid"].as_i64().unwrap() as i32);
    kill(record["child"]["pid"].as_i64().unwrap() as i32);
    up.finish();
    fs::write(root.join(".tay/state/go"), "").unwrap();
    let again = machine.at(&root, "up", &["counted"]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert_eq!(runs(&root, "runs"), 2);
}

#[test]
fn a_command_found_under_way_by_a_new_daemon_is_followed_to_its_end() {
    let machine = Machine::new();
    let root = machine.stack("commands");
    let up = machine.spawn_up(&root, &["counted"]);
    assert!(eventually(Duration::from_secs(20), || runs(&root, "runs") == 1));
    machine.kill_daemon(&root);
    up.finish();
    fs::write(root.join(".tay/state/go"), "").unwrap();
    machine.until_state(&root, "counted", Duration::from_secs(20), |state| {
        state == "ready done: it ran and exited with 0"
    });
    assert_eq!(runs(&root, "runs"), 1, "it was not run again");
}

#[test]
fn a_check_that_finds_it_done_runs_nothing_more_and_one_that_cannot_tell_fails_it() {
    let machine = Machine::new();
    let root = machine.stack("commands");
    let first = machine.at(&root, "up", &["checked"]);
    assert_eq!(first.code, 0, "{}{}", first.stdout, first.stderr);
    assert_eq!(
        first.changes(),
        [
            "checked starting attempt 1, starting",
            "checked starting attempt 1, checking",
            "checked starting attempt 1, running",
            "checked ready done: it ran and exited with 0",
        ]
    );
    let second = machine.at(&root, "up", &["checked"]);
    assert_eq!(
        second.last_change("checked"),
        "ready done: its check exited with 0"
    );
    assert_eq!(runs(&root, "checked-done"), 1, "the run ran once");
    let unsure = machine.at(&root, "up", &["unsure"]);
    assert_eq!(unsure.code, 1);
    let failed = unsure.last_change("unsure");
    assert!(
        failed.starts_with("failed its check exited with 2 at ")
            && failed.ends_with(": whether it is done cannot be told, so `run` did not run"),
        "{failed}"
    );
    assert!(!root.join(".tay/state/unsure-ran").exists());
}

#[test]
fn the_values_a_command_writes_are_kept_and_given_and_a_secret_one_with_the_secrets() {
    let machine = Machine::new();
    let root = machine.stack("commands");
    let up = machine.at(&root, "up", &["issued"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let issued = root.join(".tay/state/files/issued/issued");
    assert_eq!(fs::read_to_string(&issued).unwrap(), "id 42\n");
    // It read its request on its standard input, with its environment.
    let log = crate::harness::log(&root, "issuer");
    assert!(log.contains("| issuing for: alice (hello)"), "{log}");
    let kept = fs::read_to_string(root.join(".tay/state/units/issuer/values.json")).unwrap();
    assert!(
        kept.contains("\"id\": 42") && !kept.contains("tok-"),
        "{kept}"
    );
    let secrets = root.join(".tay/state/secrets/issuer.produced");
    assert!(
        fs::read_to_string(&secrets)
            .unwrap()
            .contains("tok-alice-secret")
    );
    assert_eq!(
        fs::metadata(&secrets).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // A daemon started afresh gives what was kept.
    machine.kill_daemon(&root);
    fs::remove_file(&issued).unwrap();
    let status = machine.at(&root, "status", &["--json"]).json();
    assert_eq!(
        Machine::state(&status, "issuer"),
        "ready done: it ran and exited with 0"
    );
    let again = machine.at(&root, "up", &["issued"]);
    assert_eq!(again.code, 0, "{}", again.stdout);
    assert_eq!(fs::read_to_string(&issued).unwrap(), "id 42\n");
}

#[test]
fn every_file_under_outputs_must_exist_once_a_command_is_done() {
    let machine = Machine::new();
    let root = machine.stack("commands");
    let built = machine.at(&root, "up", &["builder"]);
    assert_eq!(built.code, 0, "{}{}", built.stdout, built.stderr);
    let missing = machine.at(&root, "up", &["no-output"]);
    assert_eq!(missing.code, 1);
    assert_eq!(
        missing.last_change("no-output"),
        format!(
            "failed done: it ran and exited with 0, and `outputs.missing`, `{}`, does not exist",
            root.join(".tay/state/never-written").display()
        )
    );
}

#[test]
fn a_run_that_writes_no_values_gives_none_though_an_earlier_one_did() {
    let machine = Machine::new();
    let root = machine.stack("commands");
    let first = machine.at(&root, "up", &["once-values"]);
    assert_eq!(first.code, 0, "{}{}", first.stdout, first.stderr);
    let second = machine.at(&root, "up", &["once-values"]);
    assert_eq!(second.code, 1, "{}", second.stdout);
    assert_eq!(
        second.last_change("once-values"),
        "failed done: it ran and exited with 0, but it gave no value for `id`, which it lists under `produces`"
    );
}

#[test]
fn a_command_found_done_whose_secret_values_are_not_kept_fails_and_wiping_it_runs_it_again() {
    let machine = Machine::new();
    let root = machine.stack("commands");
    assert_eq!(machine.at(&root, "up", &["minter"]).code, 0);
    let again = machine.at(&root, "up", &["minter"]);
    assert_eq!(
        again.last_change("minter"),
        "ready done: its check exited with 0"
    );
    // Its secret value kept where an older tay kept it, which is not read.
    let secrets = root.join(".tay/state/secrets");
    fs::rename(secrets.join("minter.produced"), secrets.join("minter")).unwrap();
    let up = machine.at(&root, "up", &["minter"]);
    assert_eq!(up.code, 1, "{}", up.stdout);
    assert_eq!(
        up.last_change("minter"),
        "failed done: its check exited with 0, but no value for `token` is kept from when it last ran: `tay wipe minter --yes` runs it again"
    );
    let wipe = machine.at(&root, "wipe", &["minter", "--yes"]);
    assert_eq!(wipe.code, 0, "{}{}", wipe.stdout, wipe.stderr);
    let up = machine.at(&root, "up", &["minter"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(
        fs::read_to_string(secrets.join("minter.produced"))
            .unwrap()
            .contains("minted")
    );
}
