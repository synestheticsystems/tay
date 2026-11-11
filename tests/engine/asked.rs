//! What an environment was asked for stays asked: the profile, each `--set`
//! and the units named on `tay up` are remembered, the profile winning over
//! the one the environment's own file names, so that a later plain
//! `tay up`, a `tay status`, a `tay plan` and a daemon started afresh all
//! see the same stack. A `tay up` hands the daemon only what it adds, which
//! the daemon joins with what is remembered when it takes the change, so a
//! `tay set` made meanwhile is not lost.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::thread::JoinHandle;

use rustix::fs::{FlockOperation, flock};
use serde_json::{Value, json};

use crate::harness::Machine;

fn greeting(root: &std::path::Path) -> String {
    fs::read_to_string(root.join(".tay/state/files/greeting/greeting")).unwrap()
}

#[test]
fn the_profile_and_each_set_are_remembered_by_every_later_command_and_daemon() {
    let machine = Machine::new();
    let root = machine.stack("remembered");
    let up = machine.at(
        &root,
        "up",
        &["--profile", "loud", "--set", "settings.name=there"],
    );
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(greeting(&root), "HELLO, there\n");
    let plan = machine.at(&root, "plan", &["--json"]).json();
    assert_eq!(plan["plan"]["profile"], "loud");
    let status = machine.at(&root, "status", &[]);
    assert!(
        status
            .stdout
            .contains("  remembered: tay up --profile loud --set settings.name=there\n"),
        "{}",
        status.stdout
    );
    // A daemon started afresh plans with what was asked: the file it
    // checks is the one that was written.
    machine.kill_daemon(&root);
    let status = machine.at(&root, "status", &[]);
    assert_eq!(status.code, 0, "{}", status.stdout);
    assert!(
        status.stdout.contains("greeting  ready     written"),
        "{}",
        status.stdout
    );
    let again = machine.at(&root, "up", &[]);
    assert_eq!(again.last_change("greeting"), "ready as written before");
    // A new `--set` for the same place replaces the remembered one; the
    // profile stays.
    let up = machine.at(&root, "up", &["--set", "settings.name=again"]);
    assert_eq!(up.code, 0, "{}", up.stderr);
    assert_eq!(greeting(&root), "HELLO, again\n");
    let asked = machine.at(&root, "status", &["--json"]).json();
    assert_eq!(
        asked["environment"]["asked"],
        serde_json::json!({
            "profile": "loud",
            "sets": ["settings.name=again"],
            "units": [],
        })
    );
    // `--profile` replaces the remembered profile.
    machine.at(&root, "up", &["--profile", "quiet"]);
    assert_eq!(greeting(&root), "hello, again\n");
}

#[test]
fn the_profile_the_environments_file_names_outlives_a_wipe_and_the_command_line_wins() {
    let machine = Machine::new();
    let root = machine.stack("remembered");
    fs::create_dir(root.join(".tay")).unwrap();
    fs::write(root.join(".tay/choices.yaml"), "profile: loud\n").unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(greeting(&root), "HELLO, world\n");
    // Everything else the environment keeps goes; its file stays, and with
    // it the profile.
    let wipe = machine.at(&root, "wipe", &["--all", "--yes"]);
    assert_eq!(wipe.code, 0, "{}{}", wipe.stdout, wipe.stderr);
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(greeting(&root), "HELLO, world\n");
    let plan = machine.at(&root, "plan", &["--json"]).json();
    assert_eq!(plan["plan"]["profile"], "loud");
    // The command line wins, and is remembered.
    let quiet = machine.at(&root, "up", &["--profile", "quiet"]);
    assert_eq!(quiet.code, 0, "{}{}", quiet.stdout, quiet.stderr);
    assert_eq!(greeting(&root), "hello, world\n");
    let again = machine.at(&root, "up", &[]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert_eq!(greeting(&root), "hello, world\n");
    let status = machine.at(&root, "status", &[]);
    assert!(
        status
            .stdout
            .contains("  remembered: tay up --profile quiet\n"),
        "{}",
        status.stdout
    );
}

#[test]
fn the_units_named_are_remembered_and_a_secret_set_is_kept_for_this_account_and_never_shown() {
    let machine = Machine::new();
    let root = machine.stack("remembered");
    let up = machine.at(
        &root,
        "up",
        &["token-file", "--set", "settings.token=hunter2"],
    );
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let asked = root.join(".tay/state/asked");
    let mode = fs::metadata(&asked).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    // A plain `tay up` brings up the unit named before, and only it.
    let plain = machine.at(&root, "up", &[]);
    assert_eq!(
        plain.changes(),
        [
            "token-file starting making it",
            "token-file ready as written before"
        ]
    );
    let status = machine.at(&root, "status", &[]);
    assert!(
        status
            .stdout
            .contains("  remembered: tay up token-file --set settings.token=<secret>\n"),
        "{}",
        status.stdout
    );
    let json = machine.at(&root, "status", &["--json"]);
    for printed in [&status.stdout, &json.stdout, &plain.stdout] {
        assert!(!printed.contains("hunter2"), "{printed}");
    }
    let plan = machine.at(&root, "plan", &["--json"]).json();
    let planned: Vec<&str> = plan["plan"]["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|unit| unit["name"].as_str().unwrap())
        .collect();
    assert_eq!(planned, ["token-file"], "only the unit named is planned");
}

#[test]
fn set_and_unset_change_what_is_remembered_and_touch_no_unit_until_the_next_up() {
    let machine = Machine::new();
    let root = machine.stack("remembered");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(greeting(&root), "hello, world\n");
    let set = machine.at(
        &root,
        "set",
        &["settings.name=there", "settings.greeting=hi"],
    );
    assert_eq!(set.code, 0, "{}{}", set.stdout, set.stderr);
    assert_eq!(
        set.stdout,
        "set      settings.name=there\nset      settings.greeting=hi\n"
    );
    // No unit is touched: the file is written anew only by the next up.
    assert_eq!(greeting(&root), "hello, world\n");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(greeting(&root), "hi, there\n");
    // Forgetting a place lets the stack's value hold there again.
    let unset = machine.at(&root, "unset", &["settings.greeting"]);
    assert_eq!(unset.code, 0, "{}{}", unset.stdout, unset.stderr);
    assert_eq!(unset.stdout, "set      settings.name=there\n");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(greeting(&root), "hello, there\n");
    // A place with nothing remembered, and a value given, are errors, and
    // a set the stack does not plan with is not remembered.
    let again = machine.at(&root, "unset", &["settings.greeting"]);
    assert_eq!(
        (again.code, again.stderr.as_str()),
        (
            2,
            "tay unset: nothing is remembered for `settings.greeting`, so nothing was forgotten\n"
        )
    );
    let valued = machine.at(&root, "unset", &["settings.name=there"]);
    assert_eq!(valued.code, 2);
    assert!(!valued.stderr.contains("there"), "{}", valued.stderr);
    let bad = machine.at(&root, "set", &["settings.colour=red"]);
    assert_eq!(bad.code, 2, "{}{}", bad.stdout, bad.stderr);
    assert!(
        bad.stderr
            .ends_with("tay set: 1 error in the definitions; nothing was remembered.\n"),
        "{}",
        bad.stderr
    );
    let listed = machine.at(&root, "set", &[]);
    assert_eq!(listed.stdout, "set      settings.name=there\n");
}

#[test]
fn set_with_no_daemon_starts_none_and_never_prints_a_secret_settings_value() {
    let machine = Machine::new();
    let root = machine.stack("remembered");
    // Never brought up: what is set is remembered for the first `tay up`.
    let set = machine.at(&root, "set", &["settings.token=hunter2"]);
    assert_eq!(set.code, 0, "{}{}", set.stdout, set.stderr);
    assert_eq!(set.stdout, "set      settings.token=<secret>\n");
    let json = machine.at(&root, "set", &["--json"]);
    assert_eq!(
        json.json()["asked"],
        serde_json::json!({ "profile": null, "sets": ["settings.token=<secret>"], "units": [] })
    );
    let mode = fs::metadata(root.join(".tay/state/asked"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(machine.at(&root, "up", &["token-file"]).code, 0);
    let written = fs::read_to_string(root.join(".tay/state/files/token-file/token-file")).unwrap();
    assert_eq!(written, "hunter2\n");
    // Down, with no daemon: none is started to remember.
    assert_eq!(machine.at(&root, "down", &[]).code, 0);
    let unset = machine.at(&root, "unset", &["settings.token"]);
    assert_eq!(unset.code, 0, "{}{}", unset.stdout, unset.stderr);
    assert_eq!(unset.stdout, "units    token-file\n");
    assert!(machine.daemon(&root).is_none(), "no daemon was started");
    for printed in [&set.stdout, &set.stderr, &json.stdout, &unset.stdout] {
        assert!(!printed.contains("hunter2"), "{printed}");
    }
}

/// A daemon of this tay played by the test: it holds the environment's
/// lock, says who it is, and answers the first change it is asked for as
/// done at once. The thread gives the body of that request.
fn stand_in(machine: &Machine, root: &Path) -> JoinHandle<Value> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".tay/daemon.lock"))
        .unwrap();
    flock(&lock, FlockOperation::NonBlockingLockExclusive).unwrap();
    let socket = machine.socket(root);
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap();
    let about = json!({
        "tay": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "root": root,
        "id": machine.id(root),
        "started": 0,
    })
    .to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(&stream);
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let change = request.starts_with("POST ");
            let answer = match change {
                true => "{\"event\":\"decided\",\"decision\":{\"decision\":\"ready\"},\"seconds\":0.0,\"took\":[]}\n".to_string(),
                false => about.clone(),
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                answer.len()
            )
            .unwrap();
            if change {
                fs::remove_file(&socket).unwrap();
                drop(lock);
                return serde_json::from_slice(&body).unwrap();
            }
        }
        unreachable!("the listener takes connections until it is closed")
    })
}

#[test]
fn tay_up_hands_the_daemon_only_what_its_command_line_adds() {
    let machine = Machine::new();
    let root = machine.stack("remembered");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(machine.at(&root, "down", &[]).code, 0);
    assert_eq!(machine.at(&root, "set", &["settings.name=there"]).code, 0);
    let daemon = stand_in(&machine, &root);
    let up = machine.at(&root, "up", &["greeting", "--set", "settings.greeting=hi"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let sent = daemon.join().unwrap();
    assert_eq!(
        (&sent["profile"], &sent["sets"], &sent["units"]),
        (
            &json!(null),
            &json!(["settings.greeting=hi"]),
            &json!(["greeting"])
        )
    );
}

#[test]
fn the_daemon_joins_what_tay_up_adds_with_what_is_remembered_when_it_takes_the_change() {
    let machine = Machine::new();
    let root = machine.stack("remembered");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    // Remembered after the `tay up` below read what is remembered, and
    // before the daemon takes its change.
    assert_eq!(machine.at(&root, "set", &["settings.name=there"]).code, 0);
    let answer = machine.ask_daemon(
        &root,
        "/v1/up",
        r#"{"profile":null,"sets":["settings.greeting=hi"],"units":[],"home":null}"#,
    );
    assert!(answer.contains(r#""decision":"ready""#), "{answer}");
    assert_eq!(greeting(&root), "hi, there\n");
    let listed = machine.at(&root, "set", &[]);
    assert_eq!(
        listed.stdout,
        "set      settings.name=there\nset      settings.greeting=hi\n"
    );
}
