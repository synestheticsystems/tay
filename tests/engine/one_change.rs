//! One change at a time. A second `tay up` that asks for the same joins the
//! first and gets the same result; one that asks for something else is
//! refused, saying which change runs and since when; `tay down` is always
//! accepted and cancels what runs.
//!
//! To hold a change under way, the test holds the lock on the machine's
//! list of environments, through which every port is chosen: the change
//! waits on it while it makes its port.

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::Duration;

use rustix::fs::{FlockOperation, flock};

use crate::harness::{Machine, eventually};

/// The lock on the machine's list, held until it is dropped.
fn hold_the_list(machine: &Machine) -> File {
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

/// A stack brought up once, so that its daemon runs and nothing more is
/// needed of the list but choosing ports.
fn up_once(machine: &Machine) -> std::path::PathBuf {
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    root
}

fn starting_port(root: &Path, machine: &Machine) -> crate::harness::Following {
    let mut up = machine.spawn_up(root, &[]);
    up.until("config-port  starting");
    up
}

#[test]
fn a_second_up_joins_the_first_and_gets_the_same_result_and_another_change_is_refused() {
    let machine = Machine::new();
    let root = up_once(&machine);
    let list = hold_the_list(&machine);
    let first = starting_port(&root, &machine);
    let mut second = machine.spawn_up(&root, &[]);
    second.until("config-port  starting");
    let other = machine.at(&root, "up", &["config-port"]);
    assert_eq!(other.code, 4, "{}{}", other.stdout, other.stderr);
    assert!(
        other
            .stderr
            .starts_with("tay up: another change is running: `tay up`, since 2"),
        "{}",
        other.stderr
    );
    assert!(other.stderr.trim_end().ends_with("UTC"), "{}", other.stderr);
    drop(list);
    let (first, second) = (first.finish(), second.finish());
    assert_eq!(
        (first.code, second.code),
        (0, 0),
        "{}{}",
        first.stderr,
        second.stderr
    );
    assert_eq!(first.stdout, second.stdout);
    assert!(
        first
            .stdout
            .ends_with("tay up: everything asked for is ready.\n")
    );
}

#[test]
fn down_is_accepted_while_up_runs_and_cancels_it() {
    let machine = Machine::new();
    let root = up_once(&machine);
    let list = hold_the_list(&machine);
    let up = starting_port(&root, &machine);
    let mut down = machine.command(&root, &["down"]);
    down.stdout(std::process::Stdio::piped());
    let down = down.spawn().unwrap();
    // Once the daemon has taken `tay down`, it waits for the port being
    // made, and then stops everything.
    let log = root.join(".tay/logs/tay/daemon.log");
    assert!(eventually(Duration::from_secs(10), || {
        std::fs::read_to_string(&log).is_ok_and(|log| log.contains("change: tay down"))
    }));
    drop(list);
    let up = up.finish();
    let down = down.wait_with_output().unwrap();
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    assert!(
        up.stdout.ends_with("tay up: cancelled by `tay down`.\n"),
        "{}",
        up.stdout
    );
    // The file that waited on the port was never started.
    assert!(
        !up.stdout.contains("config       starting"),
        "{}",
        up.stdout
    );
    assert_eq!(down.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&down.stdout).ends_with("tay down: nothing is left running.\n")
    );
    assert!(machine.daemon_pid(&root).is_none());
}

#[test]
fn an_up_that_joins_after_the_decision_gets_the_same_result_and_another_is_refused_while_the_change_goes_on()
 {
    let machine = Machine::new();
    let root = machine.stack("made-and-run");
    assert_eq!(machine.at(&root, "up", &[]).code, 1);
    machine.settled(&root);
    // `search` fails at once, which decides the change, while `site-port`
    // waits on the list.
    let list = hold_the_list(&machine);
    let first = machine.at(&root, "up", &[]);
    assert_eq!(first.code, 1, "{}{}", first.stdout, first.stderr);
    let joined = machine.at(&root, "up", &[]);
    assert_eq!(joined.code, 1, "{}{}", joined.stdout, joined.stderr);
    assert_eq!(joined.stdout, first.stdout);
    let other = machine.at(&root, "up", &["site-port"]);
    assert_eq!(other.code, 4, "{}", other.stderr);
    drop(list);
}

/// A pipe in `root` that a unit's template is read from: each write lets
/// one reading of it, and so one planning of the stack, go on.
struct HeldPlan {
    pipe: std::path::PathBuf,
}

impl HeldPlan {
    fn new(root: &Path) -> HeldPlan {
        let pipe = root.join("notice.fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap();
        assert!(made.success());
        HeldPlan { pipe }
    }

    /// Lets the next planning go on, with `content` as the template's text,
    /// once something reads it; a planning that never comes fails the test
    /// rather than holding it for ever.
    fn release(&self, content: &'static str) -> std::thread::JoinHandle<()> {
        use rustix::fs::{Mode, OFlags, open};
        use std::io::Write;
        let pipe = self.pipe.clone();
        std::thread::spawn(move || {
            let until = std::time::Instant::now() + Duration::from_secs(20);
            let writer = loop {
                // Opening a pipe to write without waiting fails while
                // nothing reads it.
                match open(&pipe, OFlags::WRONLY | OFlags::NONBLOCK, Mode::empty()) {
                    Ok(writer) => break writer,
                    Err(rustix::io::Errno::NXIO) if std::time::Instant::now() < until => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(errno) => panic!("nothing planned within 20 seconds: {errno}"),
                }
            };
            std::fs::File::from(writer)
                .write_all(content.as_bytes())
                .unwrap();
        })
    }
}

fn logged(root: &Path, line: &str) -> bool {
    let log = root.join(".tay/logs/tay/daemon.log");
    eventually(Duration::from_secs(10), || {
        std::fs::read_to_string(&log).is_ok_and(|log| log.contains(line))
    })
}

#[test]
fn down_while_up_is_still_planned_lets_nothing_start() {
    let machine = Machine::new();
    let root = machine.stack("held-plan");
    let held = HeldPlan::new(&root);
    // The command plans first, then the daemon plans again.
    let command = held.release("for the command\n");
    let up = machine.spawn_up(&root, &["notice"]);
    command.join().unwrap();
    assert!(logged(&root, "change: tay up notice"));
    let mut down = machine.command(&root, &["down"]);
    down.stdout(std::process::Stdio::piped());
    let down = down.spawn().unwrap();
    assert!(logged(&root, "change: tay down"));
    held.release("for the daemon\n").join().unwrap();
    let up = up.finish();
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    assert_eq!(up.stdout, "tay up: cancelled by `tay down`.\n");
    assert_eq!(down.wait_with_output().unwrap().status.code(), Some(0));
}

#[test]
fn definitions_that_have_errors_when_the_daemon_plans_them_start_nothing() {
    let machine = Machine::new();
    let root = machine.stack("held-plan");
    let held = HeldPlan::new(&root);
    let command = held.release("for the command\n");
    let up = machine.spawn_up(&root, &["notice"]);
    command.join().unwrap();
    assert!(logged(&root, "change: tay up notice"));
    held.release("for the daemon: ${nobody.value}\n")
        .join()
        .unwrap();
    let up = up.finish();
    assert_eq!(up.code, 2, "{}{}", up.stdout, up.stderr);
    assert!(up.stdout.is_empty(), "{}", up.stdout);
    assert!(
        up.stderr
            .ends_with("tay up: the definitions have errors now; nothing more was started.\n"),
        "{}",
        up.stderr
    );
    assert!(
        up.stderr.contains("there is no unit `nobody`"),
        "{}",
        up.stderr
    );
}

#[test]
fn an_up_cancelled_while_its_last_unit_is_made_ends_cancelled() {
    let machine = Machine::new();
    let root = up_once(&machine);
    let list = hold_the_list(&machine);
    // The unit asked for is the one being made when `tay down` comes.
    let mut up = machine.spawn_up(&root, &["config-port"]);
    up.until("config-port  starting");
    let mut down = machine.command(&root, &["down"]);
    down.stdout(std::process::Stdio::piped());
    let down = down.spawn().unwrap();
    assert!(logged(&root, "change: tay down"));
    drop(list);
    let up = up.finish();
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    assert!(
        up.stdout.ends_with("tay up: cancelled by `tay down`.\n"),
        "{}",
        up.stdout
    );
    assert_eq!(down.wait_with_output().unwrap().status.code(), Some(0));
}

#[test]
fn set_and_unset_are_refused_while_another_change_runs() {
    let machine = Machine::new();
    let root = up_once(&machine);
    assert_eq!(machine.at(&root, "set", &["label=first"]).code, 0);
    let list = hold_the_list(&machine);
    let up = starting_port(&root, &machine);
    for (command, word) in [("set", "label=second"), ("unset", "label")] {
        let refused = machine.at(&root, command, &[word]);
        assert_eq!(refused.code, 4, "{}{}", refused.stdout, refused.stderr);
        assert!(
            refused.stderr.starts_with(&format!(
                "tay {command}: another change is running: `tay up`, since 2"
            )),
            "{}",
            refused.stderr
        );
    }
    drop(list);
    assert_eq!(up.finish().code, 0);
    let listed = machine.at(&root, "set", &[]);
    assert_eq!(listed.stdout, "set      label=first\n");
}

#[test]
fn restart_is_refused_while_another_change_runs() {
    let machine = Machine::new();
    let root = up_once(&machine);
    let list = hold_the_list(&machine);
    let up = starting_port(&root, &machine);
    let refused = machine.at(&root, "restart", &["config"]);
    assert_eq!(refused.code, 4, "{}{}", refused.stdout, refused.stderr);
    assert!(
        refused
            .stderr
            .starts_with("tay restart: another change is running: `tay up`, since 2"),
        "{}",
        refused.stderr
    );
    drop(list);
    assert_eq!(up.finish().code, 0);
}
