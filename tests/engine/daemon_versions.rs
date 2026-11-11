//! A command and a daemon of different versions: an older daemon is asked
//! to exit, which leaves the stack as it is, and the command starts its
//! own, or, when a service manager runs the older one, waits for the
//! service manager to start the new one; a newer one is left alone. The
//! other daemon is played here by a stand-in that holds the environment's
//! lock and answers on its socket as a daemon of that version would, with a
//! process of its own whose number it gives.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use rustix::fs::{FlockOperation, flock};
use rustix::process::Signal;
use serde_json::Value;

use crate::harness::{Machine, eventually};

/// How long a test waits for a command that waits for a service manager,
/// which waits thirty seconds at most.
const WAITING: Duration = Duration::from_secs(60);

/// A daemon of another version, holding the environment's lock.
struct OtherDaemon {
    /// The paths it was asked for, in order.
    asked: mpsc::Receiver<String>,
    serving: JoinHandle<()>,
}

impl OtherDaemon {
    /// One started detached, of a tay that does not say whether a service
    /// manager runs it.
    fn start(machine: &Machine, root: &Path, version: &str) -> OtherDaemon {
        OtherDaemon::answering(machine, root, version, false)
    }

    /// One a service manager runs, as `tay serve`.
    fn served(machine: &Machine, root: &Path, version: &str) -> OtherDaemon {
        OtherDaemon::answering(machine, root, version, true)
    }

    fn answering(machine: &Machine, root: &Path, version: &str, served: bool) -> OtherDaemon {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join(".tay/daemon.lock"))
            .unwrap();
        flock(&lock, FlockOperation::NonBlockingLockExclusive).unwrap();
        let listener = machine.listen_as_daemon(root);
        let process = Command::new("sleep").arg("60").spawn().unwrap();
        let mut about = serde_json::json!({
            "tay": version,
            "pid": process.id(),
            "root": root,
            "id": machine.id(root),
            "started": 0,
        });
        if served {
            about["served"] = true.into();
        }
        let about = about.to_string();
        let (told, asked) = mpsc::channel();
        let serving = std::thread::spawn(move || serve(listener, lock, process, &about, &told));
        OtherDaemon { asked, serving }
    }

    fn asked(self) -> Vec<String> {
        self.serving.join().unwrap();
        self.asked.try_iter().collect()
    }
}

/// Answers who it is until it is asked to exit, then stops listening,
/// removes its socket, lets go of the lock and ends its process, as a
/// daemon does.
fn serve(
    listener: UnixListener,
    lock: File,
    mut process: Child,
    about: &str,
    told: &mpsc::Sender<String>,
) {
    let socket = listener
        .local_addr()
        .unwrap()
        .as_pathname()
        .unwrap()
        .to_path_buf();
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let mut request = String::new();
        BufReader::new(&stream).read_line(&mut request).unwrap();
        let path = request.split(' ').nth(1).unwrap_or_default().to_string();
        told.send(path.clone()).unwrap();
        let (status, body) = match path.as_str() {
            "/v1/daemon" => ("200 OK", about.to_string()),
            "/v1/exit" => ("200 OK", r#"{"problem":"exiting"}"#.to_string()),
            _ => ("404 Not Found", r#"{"problem":"not here"}"#.to_string()),
        };
        write!(
            stream,
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        if path == "/v1/exit" || path == "/v1/up" {
            break;
        }
    }
    drop(listener);
    std::fs::remove_file(socket).unwrap();
    drop(lock);
    process.kill().unwrap();
    process.wait().unwrap();
}

/// Asks the stand-in at `root` for a change it does not make, which ends
/// it, as asking a real daemon to exit would not.
fn end(machine: &Machine, root: &Path) {
    let mut stream = std::os::unix::net::UnixStream::connect(machine.socket(root)).unwrap();
    write!(stream, "POST /v1/up HTTP/1.1\r\nHost: tay\r\n\r\n").unwrap();
    // Its answer is read, so that it is not written to no one.
    std::io::Read::read_to_string(&mut stream, &mut String::new()).unwrap();
}

/// Who the daemon at `root` says it is.
fn who(machine: &Machine, root: &Path) -> Value {
    let mut stream = std::os::unix::net::UnixStream::connect(machine.socket(root)).unwrap();
    write!(
        stream,
        "GET /v1/daemon HTTP/1.1\r\nHost: tay\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    std::io::Read::read_to_string(&mut stream, &mut answer).unwrap();
    serde_json::from_str(answer.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

/// A stack brought up and down once, so that it has its id and folders.
fn known_stack(machine: &Machine) -> std::path::PathBuf {
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(machine.at(&root, "down", &[]).code, 0);
    root
}

#[test]
fn a_command_asks_an_older_daemon_to_exit_and_starts_its_own() {
    let machine = Machine::new();
    let root = known_stack(&machine);
    let older = OtherDaemon::start(&machine, &root, "0.0.1");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(older.asked(), ["/v1/daemon", "/v1/exit"]);
    let daemon = machine.daemon(&root).expect("a daemon of this tay answers");
    assert_eq!(daemon["tay"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_command_leaves_a_newer_daemon_alone_and_says_so() {
    let machine = Machine::new();
    let root = known_stack(&machine);
    let newer = OtherDaemon::start(&machine, &root, "99.0.0");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 5);
    assert_eq!(
        up.stderr,
        format!(
            "tay up: the daemon is of tay 99.0.0, and this tay is {}: run this command with a tay of 99.0.0\n",
            env!("CARGO_PKG_VERSION")
        )
    );
    // It was never asked to exit.
    end(&machine, &root);
    assert_eq!(newer.asked(), ["/v1/daemon", "/v1/up"]);
}

#[test]
fn a_command_newer_than_a_served_daemon_waits_for_the_service_manager_to_start_the_new_one() {
    let machine = Machine::new();
    let root = known_stack(&machine);
    let older = OtherDaemon::served(&machine, &root, "0.0.1");
    let up = machine.spawn_up(&root, &[]);
    assert_eq!(older.asked(), ["/v1/daemon", "/v1/exit"]);
    // It starts no daemon of its own, which the service manager would not
    // know of.
    assert!(!eventually(Duration::from_secs(2), || {
        machine.daemon(&root).is_some()
    }));
    // The service manager starts the service again, from the same path.
    let mut served = machine.spawn(&root, "serve", &[]);
    let up = up.finish_within(WAITING);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    served.until(": it took ");
    let daemon = who(&machine, &root);
    assert_eq!(daemon["served"], true);
    assert_eq!(daemon["pid"], served.pid());
    served.signal(Signal::TERM);
    assert_eq!(served.finish_within(WAITING).code, 0);
}

#[test]
fn a_command_newer_than_a_served_daemon_says_so_when_no_new_one_is_started() {
    let machine = Machine::new();
    let root = known_stack(&machine);
    let older = OtherDaemon::served(&machine, &root, "0.0.1");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 5, "{}{}", up.stdout, up.stderr);
    assert_eq!(
        up.stderr,
        format!(
            "tay up: the daemon of tay 0.0.1, which a service manager runs, was asked to exit for this tay {}, and none has answered in its place within 30 seconds: start its service again, with this tay at the path it runs\n",
            env!("CARGO_PKG_VERSION")
        )
    );
    assert_eq!(older.asked(), ["/v1/daemon", "/v1/exit"]);
    assert!(machine.daemon(&root).is_none());
}

#[test]
fn a_command_says_so_at_once_when_the_service_manager_starts_the_older_daemon_again() {
    let machine = Machine::new();
    let root = known_stack(&machine);
    let older = OtherDaemon::served(&machine, &root, "0.0.1");
    let up = machine.spawn_up(&root, &[]);
    assert_eq!(older.asked(), ["/v1/daemon", "/v1/exit"]);
    let again = OtherDaemon::served(&machine, &root, "0.0.1");
    let up = up.finish_within(WAITING);
    assert_eq!(up.code, 5, "{}{}", up.stdout, up.stderr);
    assert_eq!(
        up.stderr,
        format!(
            "tay up: the service manager started a daemon of tay 0.0.1 in place of the one asked to exit, not one of this tay {}: put this tay at the path its service runs, and start the service again\n",
            env!("CARGO_PKG_VERSION")
        )
    );
    end(&machine, &root);
    assert_eq!(again.asked(), ["/v1/daemon", "/v1/up"]);
}

#[test]
fn serve_leaves_a_newer_daemon_alone_and_says_so() {
    let machine = Machine::new();
    let root = known_stack(&machine);
    let newer = OtherDaemon::start(&machine, &root, "99.0.0");
    let served = machine.at(&root, "serve", &[]);
    assert_eq!(served.code, 5);
    assert_eq!(
        served.stderr,
        format!(
            "tay serve: the daemon is of tay 99.0.0, and this tay is {}: run this command with a tay of 99.0.0\n",
            env!("CARGO_PKG_VERSION")
        )
    );
    end(&machine, &root);
    assert_eq!(newer.asked(), ["/v1/daemon", "/v1/up"]);
}
