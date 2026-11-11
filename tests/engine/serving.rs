//! `tay serve`: the environment's daemon in the foreground, for a service
//! manager. It brings up what the environment remembers being asked for,
//! whatever was recorded of whether it is meant to be up, tells a service
//! manager that waits for it once it answers, prints each change of
//! state, and takes everything down when told to stop by
//! `SIGTERM` or `SIGINT`, or by `tay down` through its socket, exiting with
//! 0. A hang-up does not stop it. Killed, it leaves the stack running, and
//! the next daemon adopts it. It takes the place of a daemon that runs, and
//! exits with another status only when it could not be the daemon or could
//! not go on, saying why.

use std::fs;
use std::io::Write;
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::path::Path;
use std::time::Duration;

use rustix::process::Signal;

use crate::harness::{Following, Machine, alive, child, eventually, record};

/// How long a served daemon has to take the environment down and exit.
const ENDS: Duration = Duration::from_secs(30);

/// `tay serve` at `root`, read until its bring-up says how long it took.
fn serve(machine: &Machine, root: &Path) -> Following {
    let mut serve = machine.spawn(root, "serve", &[]);
    serve.until(": it took ");
    serve
}

/// The program of each of `units`, by its process, and the attempt it is.
fn programs(root: &Path, units: &[&str]) -> Vec<(i32, u64)> {
    units
        .iter()
        .map(|unit| {
            let attempt = record(root, unit)["attempt"].as_u64().unwrap();
            (child(root, unit), attempt)
        })
        .collect()
}

/// Nothing `served` brought up was started again: each unit was found as
/// it runs.
fn assert_found_as_they_run(served: &Following) {
    let starting = served
        .read
        .lines()
        .filter(|line| line.split_whitespace().nth(1) == Some("starting"));
    for line in starting {
        assert!(line.ends_with("found under way"), "{}", served.read);
    }
}

fn intent(root: &Path) -> String {
    fs::read_to_string(root.join(".tay/state/intent")).unwrap()
}

#[test]
fn it_tells_a_service_manager_that_waits_for_it_once_it_answers_and_not_before() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    // A socket of the test's own stands in for systemd's.
    let notify = machine.path("notify");
    let systemd = UnixDatagram::bind(&notify).unwrap();
    // While the machine's list is held, it cannot make the environment its
    // own, let alone answer for it.
    fs::create_dir_all(machine.path("state/tay")).unwrap();
    let list = machine.hold_the_list();
    let mut command = machine.command(&root, &["serve", "--root", root.to_str().unwrap()]);
    command.env("NOTIFY_SOCKET", &notify);
    let mut served = machine.follow(command);
    let mut notice = [0; 64];
    systemd
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    assert!(
        systemd.recv(&mut notice).is_err(),
        "a notice before it answers"
    );
    drop(list);
    systemd.set_read_timeout(Some(ENDS)).unwrap();
    let read = systemd.recv(&mut notice).expect("the notice comes");
    assert_eq!(&notice[..read], b"READY=1");
    // It answers already, where it wrote that it listens.
    let socket = fs::read_to_string(root.join(".tay/daemon.socket")).unwrap();
    let mut stream = UnixStream::connect(socket.trim_end()).unwrap();
    write!(
        stream,
        "GET /v1/daemon HTTP/1.1\r\nHost: tay\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    std::io::Read::read_to_string(&mut stream, &mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    let about: serde_json::Value =
        serde_json::from_str(answer.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(about["pid"], served.pid());
    served.until(": it took ");
    // It is told once.
    systemd
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    assert!(systemd.recv(&mut notice).is_err(), "a second notice");
    served.signal(Signal::TERM);
    assert_eq!(served.finish_within(ENDS).code, 0);
}

#[test]
fn it_brings_up_an_environment_that_is_down_and_a_plain_up_joins_its_bring_up() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(machine.at(&root, "down", &[]).code, 0);
    assert_eq!(intent(&root), "down\n");
    let list = machine.hold_the_list();
    let mut served = machine.spawn(&root, "serve", &[]);
    served.until("config-port  starting");
    // A plain `tay up` asks what `tay serve` began with, and joins it.
    let mut up = machine.spawn_up(&root, &[]);
    up.until("config-port  starting");
    let other = machine.at(&root, "up", &["config-port"]);
    assert_eq!(other.code, 4, "{}{}", other.stdout, other.stderr);
    assert!(
        other
            .stderr
            .starts_with("tay up: another change is running: `tay serve`, since 2"),
        "{}",
        other.stderr
    );
    drop(list);
    let up = up.finish();
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let ready = "  config       ready     as written before\n";
    assert!(
        up.before_it_took()
            .ends_with(&format!("{ready}tay up: everything asked for is ready.\n")),
        "{}",
        up.stdout
    );
    served.until(": it took ");
    assert!(
        served.read.contains(&format!(
            "{ready}tay serve: everything asked for is ready.\n"
        )),
        "{}",
        served.read
    );
    assert_eq!(intent(&root), "up\n");
    let status = machine.at(&root, "status", &["--json"]).json();
    assert_eq!(Machine::state(&status, "config"), "ready written");
    served.signal(Signal::TERM);
    assert_eq!(served.finish_within(ENDS).code, 0);
}

#[test]
fn told_to_stop_it_takes_everything_down_and_exits_with_0() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let served = serve(&machine, &root);
    for change in ["\n  api  ready     attempt ", "\n  web  ready     attempt "] {
        assert!(served.read.contains(change), "{}", served.read);
    }
    let status = machine.at(&root, "status", &["--json"]).json();
    assert_eq!(Machine::state(&status, "web"), "ready attempt 1, running");
    let programs = programs(&root, &["api", "web"]);
    served.signal(Signal::TERM);
    let ended = served.finish_within(ENDS);
    assert_eq!(ended.code, 0, "{}{}", ended.stdout, ended.stderr);
    assert!(
        ended.stdout.ends_with(
            "  web  stopped   stopped: `tay serve` was told to stop\n  api  stopped   stopped: `tay serve` was told to stop\ntay serve: nothing is left running.\n"
        ),
        "{}",
        ended.stdout
    );
    assert_eq!(ended.stderr, "");
    for (program, _) in programs {
        assert!(!alive(program));
    }
    assert!(machine.processes().is_empty());
    // Down means down: no later command brings it up unasked.
    assert_eq!(intent(&root), "down\n");
}

#[test]
fn control_c_does_what_sigterm_does() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    let served = serve(&machine, &root);
    served.signal(Signal::INT);
    let ended = served.finish_within(ENDS);
    assert_eq!(ended.code, 0, "{}{}", ended.stdout, ended.stderr);
    assert!(
        ended.stdout.ends_with(
            "stopped: `tay serve` was told to stop\ntay serve: nothing is left running.\n"
        ),
        "{}",
        ended.stdout
    );
    assert!(machine.processes().is_empty());
    assert_eq!(intent(&root), "down\n");
}

#[test]
fn tay_down_from_another_process_takes_everything_down_and_it_exits_with_0() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let served = serve(&machine, &root);
    let programs = programs(&root, &["api", "web"]);
    // `tay down` waits for the daemon's process to be gone, which this
    // test, its parent, sees to as a service manager would.
    let down = machine.spawn(&root, "down", &[]);
    let ended = served.finish_within(ENDS);
    let down = down.finish();
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    assert_eq!(ended.code, 0, "{}{}", ended.stdout, ended.stderr);
    assert!(
        ended.stdout.ends_with(
            "  api  stopped   stopped by `tay down`\ntay serve: nothing is left running.\n"
        ),
        "{}",
        ended.stdout
    );
    for (program, _) in programs {
        assert!(!alive(program));
    }
    assert!(machine.processes().is_empty());
}

#[test]
fn killed_it_leaves_the_stack_running_and_the_next_one_adopts_it_without_starting_any_again() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let first = serve(&machine, &root);
    let programs_before = programs(&root, &["api", "web"]);
    first.signal(Signal::KILL);
    assert_eq!(first.killed_within(ENDS), Signal::KILL.as_raw());
    for (program, _) in &programs_before {
        assert!(alive(*program));
    }
    let second = serve(&machine, &root);
    assert_eq!(programs(&root, &["api", "web"]), programs_before);
    assert_found_as_they_run(&second);
    second.signal(Signal::TERM);
    assert_eq!(second.finish_within(ENDS).code, 0);
}

#[test]
fn a_hang_up_is_no_signal_to_stop_and_the_daemon_carries_on() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let served = serve(&machine, &root);
    let programs_before = programs(&root, &["api", "web"]);
    served.signal(Signal::HUP);
    let log = root.join(".tay/logs/tay/daemon.log");
    assert!(eventually(ENDS, || {
        fs::read_to_string(&log)
            .unwrap()
            .contains("SIGHUP is not a signal to stop")
    }));
    assert!(alive(served.pid()));
    assert_eq!(machine.daemon_pid(&root), Some(served.pid()));
    assert_eq!(programs(&root, &["api", "web"]), programs_before);
    served.signal(Signal::TERM);
    assert_eq!(served.finish_within(ENDS).code, 0);
}

#[test]
fn it_takes_the_place_of_a_daemon_started_detached_and_nothing_restarts() {
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let detached = machine.daemon_pid(&root).unwrap();
    let programs_before = programs(&root, &["api", "web"]);
    let served = serve(&machine, &root);
    assert!(!alive(detached));
    assert_eq!(machine.daemon_pid(&root), Some(served.pid()));
    assert_eq!(programs(&root, &["api", "web"]), programs_before);
    assert_found_as_they_run(&served);
    served.signal(Signal::TERM);
    assert_eq!(served.finish_within(ENDS).code, 0);
}

#[test]
fn a_command_started_while_it_takes_the_place_of_a_daemon_waits_for_it_and_starts_none() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    // An answer under way holds the daemon that is replaced for as long as
    // it is given to finish, five seconds.
    let mut under_way = UnixStream::connect(machine.socket(&root)).unwrap();
    write!(under_way, "GET /v1/daemon HTTP/1.1\r\n").unwrap();
    let mut served = machine.spawn(&root, "serve", &[]);
    let log = root.join(".tay/logs/tay/daemon.log");
    assert!(eventually(ENDS, || {
        fs::read_to_string(&log)
            .unwrap()
            .contains("exiting: asked to, so that another daemon takes its place")
    }));
    let up = machine.spawn_up(&root, &[]).finish_within(ENDS);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    served.until(": it took ");
    assert_eq!(machine.daemon_pid(&root), Some(served.pid()));
    let started = fs::read_to_string(&log)
        .unwrap()
        .matches("  started: ")
        .count();
    assert_eq!(started, 2, "the first daemon and the served one");
    drop(under_way);
    served.signal(Signal::TERM);
    assert_eq!(served.finish_within(ENDS).code, 0);
}

#[test]
fn a_second_serve_takes_the_place_of_the_first_which_exits_saying_why() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let first = serve(&machine, &root);
    let programs_before = programs(&root, &["api", "web"]);
    // The second waits for the first's process to be gone, which this
    // test, its parent, sees to as a service manager would.
    let mut second = machine.spawn(&root, "serve", &[]);
    let first = first.finish_within(ENDS);
    second.until(": it took ");
    assert_eq!(first.code, 5, "{}{}", first.stdout, first.stderr);
    assert_eq!(
        first.stderr,
        "tay serve: it was asked to exit, so that another daemon takes its place: one of another version of tay, or another `tay serve`; the stack runs on\n"
    );
    assert_eq!(machine.daemon_pid(&root), Some(second.pid()));
    assert_eq!(programs(&root, &["api", "web"]), programs_before);
    second.signal(Signal::TERM);
    assert_eq!(second.finish_within(ENDS).code, 0);
}

#[test]
fn errors_in_the_definitions_end_it_as_they_end_tay_up_with_nothing_started() {
    let machine = Machine::new();
    for (stack, code) in [("errors-many", 2), ("too-new", 3)] {
        let root = machine.stack(stack);
        let up = machine.at(&root, "up", &[]);
        let served = machine.at(&root, "serve", &[]);
        assert_eq!((up.code, served.code), (code, code), "{}", served.stderr);
        assert_eq!(served.stderr, up.stderr.replace("tay up:", "tay serve:"));
        assert_eq!(served.stdout, "");
        assert!(machine.daemon(&root).is_none());
    }
    assert!(machine.processes().is_empty());
}
