//! The daemon: started detached by the first command that needs it, alive
//! for as long as it holds its lock, reached over a Unix socket that speaks
//! HTTP with JSON under `/v1/`, and gone after `tay down`, when its folder
//! disappears, or when its socket does.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use rustix::fs::{FlockOperation, flock};

use crate::harness::{Machine, alive, eventually};
use crate::support::Run;

/// Whether another holds the lock on the file at `path`.
fn locked(path: &Path) -> bool {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    flock(&file, FlockOperation::NonBlockingLockExclusive).is_err()
}

#[test]
fn the_first_command_that_needs_the_daemon_starts_it_detached_and_it_outlives_the_command() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let pid = machine.daemon_pid(&root).expect("a daemon answers");
    assert!(alive(pid));
    // It leads a session of its own: neither the terminal nor the shell
    // that ran the command reaches it.
    let session = rustix::process::getsid(rustix::process::Pid::from_raw(pid)).unwrap();
    assert_eq!(session.as_raw_nonzero().get(), pid);
    // A second command finds the same daemon rather than starting another.
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(machine.daemon_pid(&root), Some(pid));
}

#[test]
fn the_daemons_lifetime_is_its_lock_and_a_socket_left_by_a_killed_one_is_replaced() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let lock = root.join(".tay/daemon.lock");
    assert!(locked(&lock));
    let pid = machine.daemon_pid(&root).unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    assert!(eventually(Duration::from_secs(5), || !locked(&lock)));
    // The killed daemon left its socket; the next command replaces it.
    assert!(machine.socket(&root).exists());
    let status = machine.at(&root, "status", &[]);
    assert_eq!(status.code, 0, "{}", status.stderr);
    let again = machine.daemon_pid(&root).unwrap();
    assert_ne!(again, pid);
    assert!(locked(&lock));
}

#[test]
fn a_new_daemon_after_a_kill_finds_the_environment_as_it_is() {
    let machine = Machine::new();
    let root = machine.stack("made-and-run");
    assert_eq!(machine.at(&root, "up", &[]).code, 1);
    let before = machine.settled(&root);
    let pid = machine.daemon_pid(&root).unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    assert!(eventually(Duration::from_secs(5), || !alive(pid)));
    // The environment is meant to be up, so `tay status` starts a daemon,
    // which finds each unit again from a check.
    let after = machine.at(&root, "status", &["--json"]).json();
    assert_ne!(machine.daemon_pid(&root), Some(pid));
    let states = |status: &serde_json::Value| -> Vec<(String, String)> {
        status["units"]
            .as_array()
            .unwrap()
            .iter()
            .map(|unit| {
                (
                    unit["unit"].as_str().unwrap().to_string(),
                    unit["state"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    assert_eq!(states(&after), states(&before));
    let port = |status: &serde_json::Value| {
        status["units"]
            .as_array()
            .unwrap()
            .iter()
            .find(|unit| unit["unit"] == "site-port")
            .unwrap()["detail"]
            .clone()
    };
    assert_eq!(port(&after), port(&before));
}

#[test]
fn the_daemon_listens_on_a_socket_for_this_account_alone_speaking_http_with_json_under_v1() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let socket = machine.socket(&root);
    let id = machine.id(&root);
    // The socket is in a short folder named by the environment's id.
    assert_eq!(
        socket.parent().unwrap().file_name().unwrap().to_str(),
        Some(id.as_str())
    );
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&socket), 0o600);
    assert_eq!(mode(socket.parent().unwrap()), 0o700);
    let ask = |request: &str| {
        let mut stream = UnixStream::connect(&socket).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    };
    let answer = ask("GET /v1/daemon HTTP/1.1\r\nHost: tay\r\nConnection: close\r\n\r\n");
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(
        answer.contains("content-type: application/json"),
        "{answer}"
    );
    let body: serde_json::Value =
        serde_json::from_str(answer.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["id"], id.as_str());
    assert_eq!(body["tay"], env!("CARGO_PKG_VERSION"));
    // One started detached says that no service manager runs it.
    assert_eq!(body["served"], false);
    let elsewhere = ask("GET /v2/daemon HTTP/1.1\r\nHost: tay\r\nConnection: close\r\n\r\n");
    assert!(
        elsewhere.starts_with("HTTP/1.1 404 Not Found\r\n"),
        "{elsewhere}"
    );
}

#[test]
fn the_daemon_opens_no_tcp_port() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let pid = machine.daemon_pid(&root).unwrap();
    let lsof = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-i", "TCP"])
        .output()
        .expect("lsof, which lists a process's sockets, is installed");
    assert_eq!(String::from_utf8_lossy(&lsof.stdout), "");
}

#[test]
fn down_stops_everything_and_the_daemon_exits() {
    let machine = Machine::new();
    let root = machine.stack("made-and-run");
    machine.at(&root, "up", &[]);
    machine.settled(&root);
    let pid = machine.daemon_pid(&root).unwrap();
    let order = machine.state_order(&root);
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    assert!(
        down.stdout
            .ends_with("tay down: nothing is left running.\n")
    );
    // Every unit is stopped, in the reverse of the order units start in.
    let order: Vec<String> = order.into_iter().rev().collect();
    let stopped: Vec<String> = down
        .changes()
        .iter()
        .map(|line| {
            let (unit, rest) = line.split_once(' ').unwrap();
            assert_eq!(rest, "stopped stopped by `tay down`");
            unit.to_string()
        })
        .collect();
    assert_eq!(stopped, order);
    // The daemon has exited by the time `tay down` returns, and removed
    // its socket.
    assert!(!alive(pid));
    assert!(!machine.socket(&root).exists());
    assert!(!locked(&root.join(".tay/daemon.lock")));
    // With nothing running, `tay down` says so again and starts nothing.
    let again = machine.at(&root, "down", &[]);
    assert_eq!(again.code, 0);
    assert_eq!(again.stdout, "tay down: nothing is left running.\n");
    assert!(machine.daemon_pid(&root).is_none());
}

#[test]
fn the_daemon_exits_when_the_environments_folder_disappears() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let pid = machine.daemon_pid(&root).unwrap();
    fs::remove_dir_all(&root).unwrap();
    assert!(eventually(Duration::from_secs(5), || !alive(pid)));
}

#[test]
fn the_daemon_exits_when_its_socket_is_removed() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let pid = machine.daemon_pid(&root).unwrap();
    fs::remove_file(machine.socket(&root)).unwrap();
    assert!(eventually(Duration::from_secs(5), || !alive(pid)));
    // The environment is still meant to be up: the next command starts a
    // daemon that finds it.
    let status = machine.at(&root, "status", &[]);
    assert_eq!(status.code, 0, "{}", status.stderr);
    assert!(
        status.stdout.contains("config       ready"),
        "{}",
        status.stdout
    );
}

/// `tay COMMAND --root ROOT ARGS` run as a service manager runs a service,
/// with no `XDG_RUNTIME_DIR`, so that a daemon it starts listens in the
/// temporary folder `tmp`, where a login session's commands would not look.
fn as_a_service(machine: &Machine, tmp: &Path, root: &Path, command: &str, args: &[&str]) -> Run {
    let mut all = vec![command, "--root", root.to_str().unwrap()];
    all.extend(args);
    let mut command = machine.command(root, &all);
    command.env_remove("XDG_RUNTIME_DIR").env("TMPDIR", tmp);
    machine.run(command)
}

/// What is written in `.tay/` at `root` of the socket the daemon listens
/// on.
fn written_socket(root: &Path) -> Option<String> {
    fs::read_to_string(root.join(".tay/daemon.socket")).ok()
}

#[test]
fn a_command_finds_the_daemon_by_the_socket_it_wrote_whatever_session_either_was_started_in() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    let tmp = machine.path("tmp");
    fs::create_dir(&tmp).unwrap();
    let up = as_a_service(&machine, &tmp, &root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let uid = rustix::process::getuid().as_raw();
    let served_socket = tmp
        .join(format!("tay-{uid}"))
        .join(machine.id(&root))
        .join("daemon.sock");
    assert_eq!(
        written_socket(&root),
        Some(format!("{}\n", served_socket.display()))
    );
    let ls = as_a_service(&machine, &tmp, &root, "ls", &["--json"]);
    let pid = ls.json()["environments"][0]["daemon"]["pid"].clone();
    assert!(pid.is_number(), "{}", ls.stdout);
    // Commands of a session whose runtime folder is another reach that
    // daemon, and start none of their own.
    let status = machine.at(&root, "status", &["--json"]);
    assert_eq!(status.code, 0, "{}", status.stderr);
    assert_eq!(Machine::state(&status.json(), "config"), "ready written");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let daemon = machine.daemon(&root).expect("a daemon answers");
    assert_eq!(daemon["pid"], pid);
    assert!(!machine.socket(&root).exists());
    // And the other way about: a daemon started in the login session is
    // reached by a command run as a service.
    let down = as_a_service(&machine, &tmp, &root, "down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    assert!(!alive(pid.as_i64().unwrap() as i32));
    assert_eq!(written_socket(&root), None, "the daemon forgets it");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let status = as_a_service(&machine, &tmp, &root, "status", &[]);
    assert_eq!(status.code, 0, "{}", status.stderr);
    assert_eq!(
        written_socket(&root),
        Some(format!("{}\n", machine.socket(&root).display()))
    );
    assert!(!served_socket.exists());
}

#[test]
fn a_socket_written_by_a_daemon_that_died_does_not_stop_the_next_command_starting_one() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    let tmp = machine.path("tmp");
    fs::create_dir(&tmp).unwrap();
    assert_eq!(as_a_service(&machine, &tmp, &root, "up", &[]).code, 0);
    let stale = written_socket(&root).unwrap();
    // Killed, it leaves its socket and what it wrote of it.
    machine.kill_daemon(&root);
    assert_eq!(written_socket(&root).as_ref(), Some(&stale));
    let status = machine.at(&root, "status", &[]);
    assert_eq!(status.code, 0, "{}", status.stderr);
    assert!(
        status.stdout.contains("config       ready"),
        "{}",
        status.stdout
    );
    assert_eq!(
        written_socket(&root),
        Some(format!("{}\n", machine.socket(&root).display()))
    );
    // The runtime folder of its session may have gone with it, as a
    // login's does when the person logs out.
    machine.kill_daemon(&root);
    fs::remove_dir_all(machine.path("run")).unwrap();
    fs::create_dir(machine.path("run")).unwrap();
    let up = as_a_service(&machine, &tmp, &root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(written_socket(&root), Some(stale));
}

#[test]
fn whether_the_environment_is_meant_to_be_up_is_kept_on_disk() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    let intent = || fs::read_to_string(root.join(".tay/state/intent")).unwrap();
    machine.at(&root, "up", &[]);
    assert_eq!(intent(), "up\n");
    machine.at(&root, "down", &[]);
    assert_eq!(intent(), "down\n");
    // A daemon started while the environment is down finds nothing up.
    let status = machine.at(&root, "status", &[]);
    assert!(
        status
            .stdout
            .starts_with(&format!("{}: down\n", root.display()))
    );
    assert!(
        machine.daemon_pid(&root).is_none(),
        "status started no daemon"
    );
}

#[test]
fn a_daemon_that_cannot_be_started_or_reached_is_exit_code_5() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    // A runtime folder others may write in is not used for the socket.
    let runtime = machine.path("run/tay");
    fs::create_dir_all(&runtime).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o777)).unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 5, "{}", up.stderr);
    assert!(
        up.stderr.contains("is not this account's alone"),
        "{}",
        up.stderr
    );
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    // A socket's path is limited to about a hundred bytes.
    let deep = machine
        .path("run")
        .join("a-folder-whose-name-is-long-enough-to-push-the-socket-path-past-its-limit");
    fs::create_dir_all(&deep).unwrap();
    let mut command = machine.command(&root, &["up"]);
    command.env("XDG_RUNTIME_DIR", &deep);
    let up = machine.run(command);
    assert_eq!(up.code, 5);
    assert!(
        up.stderr
            .contains("set XDG_RUNTIME_DIR to a shorter folder"),
        "{}",
        up.stderr
    );
}

#[test]
fn a_daemon_asked_to_exit_leaves_the_stack_as_it_is() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let before = machine.settled(&root);
    let pid = machine.daemon_pid(&root).unwrap();
    let mut stream = UnixStream::connect(machine.socket(&root)).unwrap();
    stream
        .write_all(b"POST /v1/exit HTTP/1.1\r\nHost: tay\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(eventually(Duration::from_secs(5), || !alive(pid)));
    // It stopped nothing: the environment is still meant to be up, and the
    // next daemon finds every unit as it was.
    assert_eq!(
        fs::read_to_string(root.join(".tay/state/intent")).unwrap(),
        "up\n"
    );
    let after = machine.settled(&root);
    let states = |status: &serde_json::Value| -> Vec<String> {
        status["units"]
            .as_array()
            .unwrap()
            .iter()
            .map(|unit| Machine::state(status, unit["unit"].as_str().unwrap()))
            .collect()
    };
    assert_eq!(states(&after), states(&before));
}

#[test]
fn down_with_no_daemon_finds_what_runs_and_stops_it() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let pid = machine.daemon_pid(&root).unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    assert!(eventually(Duration::from_secs(5), || !alive(pid)));
    // The environment is meant to be up, so `tay down` starts a daemon to
    // find its units, and that daemon stops them and exits.
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}", down.stderr);
    assert!(
        down.changes()
            .contains(&"config stopped stopped by `tay down`".to_string())
    );
    assert!(machine.daemon_pid(&root).is_none());
    assert_eq!(
        fs::read_to_string(root.join(".tay/state/intent")).unwrap(),
        "down\n"
    );
}
