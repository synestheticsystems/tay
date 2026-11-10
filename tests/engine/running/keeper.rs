//! The keeper: each program a unit runs on the host has one, which outlives
//! the daemon, goes on writing the unit's log, and records how the program
//! ended; and the order of a start leaves no moment at which a program runs
//! that the next daemon does not know of.

use std::fs::OpenOptions;
use std::time::{Duration, Instant};

use rustix::fs::{FlockOperation, flock};

use crate::harness::{Machine, alive, child, command_of, eventually, log};

#[test]
fn a_unit_outlives_its_daemon_goes_on_logging_and_is_found_again_without_starting_again() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let up = machine.at(&root, "up", &["web"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let web = child(&root, "web");
    let ticks = || log(&root, "web").matches("| tick").count();
    // A program is ready once it has started, which may be before it has
    // written anything on a busy machine.
    assert!(eventually(Duration::from_secs(20), || ticks() > 0));
    machine.kill_daemon(&root);
    let at_kill = ticks();
    assert!(
        eventually(Duration::from_secs(20), || ticks() >= at_kill + 3),
        "it went on logging: {}",
        log(&root, "web")
    );
    assert!(alive(web), "the unit runs on with no daemon");
    // Each line has its time.
    assert!(
        log(&root, "web").lines().all(|line| line.len() > 25
            && line.as_bytes()[10] == b'T'
            && line[..24].ends_with('Z')),
        "{}",
        log(&root, "web")
    );
    // The next daemon adopts it: the same process, the same attempt.
    machine.until_state(&root, "web", Duration::from_secs(20), |state| {
        state == "ready attempt 1, running"
    });
    machine.until_state(&root, "api", Duration::from_secs(20), |state| {
        state
            == "ready attempt 1, ready when it was found again; log \"listening\" is checked again"
    });
    assert_eq!(child(&root, "web"), web);
    assert!(alive(web));
    assert!(!log(&root, "web").contains("attempt 2"));
}

#[test]
fn a_second_start_of_a_unit_that_runs_is_refused() {
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &["web"]).code, 0);
    let api = child(&root, "api");
    // The environment is asked for `spawner` alone from now; `api` runs on,
    // and a daemon started afresh plans without it.
    assert_eq!(machine.at(&root, "up", &["spawner"]).code, 0);
    machine.kill_daemon(&root);
    assert_eq!(machine.at(&root, "status", &[]).code, 0);
    let again = machine.at(&root, "up", &["api"]);
    assert_eq!(again.code, 1, "{}", again.stdout);
    assert_eq!(
        again.last_change("api"),
        "failed it is running already, under a keeper this daemon did not start"
    );
    assert_eq!(child(&root, "api"), api);
    assert!(alive(api), "the one that ran is left as it was");
    let web = child(&root, "web");
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}", down.stdout);
    assert!(!alive(api), "`tay down` stops it all the same");
    // And what runs of a unit the plan does not have at all.
    assert!(!alive(web));
    assert!(machine.processes().is_empty(), "{:?}", machine.processes());
}

/// Holds the lock on the machine's list of environments, through which
/// every port is chosen, until it is dropped: a start that needs a port
/// waits on it.
fn hold_the_list(machine: &Machine) -> std::fs::File {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(machine.path("state/tay/environments.lock"))
        .unwrap();
    flock(&lock, FlockOperation::NonBlockingLockExclusive).unwrap();
    lock
}

/// Where in a start the daemon was when it was killed, as what it left on
/// disk says.
fn point(root: &std::path::Path) -> &'static str {
    let Some(record) = written_record(root) else {
        return "before the step was written";
    };
    match (record["keeper"].is_null(), record["child"].is_null()) {
        (true, _) => "after the step was written, before the keeper wrote itself",
        (false, true) => "after the keeper wrote itself, before its child started",
        (false, false) => "after the child was started",
    }
}

fn written_record(root: &std::path::Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(root.join(".tay/state/units/ticking/record.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// When the daemon is killed: a while after the start is let go, or as soon
/// as the record says a moment of it has come.
#[derive(Clone, Copy, Debug)]
enum Moment {
    After(u64),
    /// This many microseconds after the step was written.
    StepWritten(u64),
    KeeperWritten,
    ChildStarted,
}

impl Moment {
    fn has_come(
        self,
        let_go: Instant,
        written: &mut Option<Instant>,
        root: &std::path::Path,
    ) -> bool {
        let record = || written_record(root).unwrap_or(serde_json::Value::Null);
        match self {
            Moment::After(millis) => let_go.elapsed() >= Duration::from_millis(millis),
            Moment::StepWritten(micros) => {
                if written.is_none() && written_record(root).is_some() {
                    *written = Some(Instant::now());
                }
                written.is_some_and(|at| at.elapsed() >= Duration::from_micros(micros))
            }
            Moment::KeeperWritten => !record()["keeper"].is_null(),
            Moment::ChildStarted => !record()["child"].is_null(),
        }
    }
}

#[test]
fn the_daemon_killed_at_any_point_of_a_start_leaves_nothing_the_next_daemon_does_not_know_of() {
    // The start of `ticking` waits for its port, which waits on the list's
    // lock. The daemon is killed once the lock is let go: at once, a while
    // after, or as soon as the record shows a moment of the start has come.
    let moments = [
        Moment::After(0),
        Moment::StepWritten(0),
        Moment::StepWritten(100),
        Moment::StepWritten(200),
        Moment::StepWritten(300),
        Moment::StepWritten(400),
        Moment::KeeperWritten,
        Moment::ChildStarted,
    ];
    for moment in moments {
        let machine = Machine::new();
        let root = machine.stack("services");
        // The daemon runs, and the environment is on the machine's list.
        assert_eq!(machine.at(&root, "up", &["ticks-port"]).code, 0);
        let list = hold_the_list(&machine);
        let mut up = machine.spawn_up(&root, &["ticking"]);
        up.until("ticks-port  starting");
        let daemon = machine.daemon_pid(&root).unwrap();
        drop(list);
        let let_go = Instant::now();
        let mut written = None;
        while !moment.has_come(let_go, &mut written, &root) {
            assert!(
                let_go.elapsed() < Duration::from_secs(10),
                "{moment:?} never came"
            );
        }
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(daemon).unwrap(),
            rustix::process::Signal::KILL,
        )
        .unwrap();
        // What the record said as the daemon was killed; a keeper it had
        // started goes on writing.
        let at = point(&root);
        assert!(eventually(Duration::from_secs(5), || !alive(daemon)));
        up.finish();
        // The next daemon says `ticking` runs exactly when a process of it
        // does, once it has settled on what it found.
        let mut state = String::new();
        let settled = eventually(Duration::from_secs(15), || {
            state = Machine::state(&machine.at(&root, "status", &["--json"]).json(), "ticking");
            !state.starts_with("starting")
        });
        assert!(settled, "killed {at}: `ticking` stays `{state}`");
        let runs = machine.processes().into_iter().any(|pid| {
            let command = command_of(pid);
            command.contains("ticker.sh") || command.contains("tay keep")
        });
        assert_eq!(
            state.starts_with("ready"),
            runs,
            "killed {at}: `ticking` is `{state}`, and a process of it runs: {runs}"
        );
        let down = machine.at(&root, "down", &[]);
        assert_eq!(down.code, 0, "killed {at}: {}", down.stdout);
        assert!(
            machine.processes().is_empty(),
            "killed {at}: something is left"
        );
    }
}
