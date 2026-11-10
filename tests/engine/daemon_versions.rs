//! A command and a daemon of different versions: an older daemon is asked
//! to exit, which leaves the stack as it is, and the command starts its
//! own; a newer one is left alone. The other daemon is played here by a
//! stand-in that holds the environment's lock and answers on its socket as
//! a daemon of that version would, with a process of its own whose number
//! it gives.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::mpsc;
use std::thread::JoinHandle;

use rustix::fs::{FlockOperation, flock};

use crate::harness::Machine;

/// A daemon of another version, holding the environment's lock.
struct OtherDaemon {
    /// The paths it was asked for, in order.
    asked: mpsc::Receiver<String>,
    serving: JoinHandle<()>,
}

impl OtherDaemon {
    fn start(machine: &Machine, root: &Path, version: &str) -> OtherDaemon {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join(".tay/daemon.lock"))
            .unwrap();
        flock(&lock, FlockOperation::NonBlockingLockExclusive).unwrap();
        let socket = machine.socket(root);
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let process = Command::new("sleep").arg("60").spawn().unwrap();
        let about = serde_json::json!({
            "tay": version,
            "pid": process.id(),
            "root": root,
            "id": machine.id(root),
            "started": 0,
        })
        .to_string();
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
    // It was never asked to exit; the stand-in ends once asked for a change
    // it does not make.
    let mut stream = std::os::unix::net::UnixStream::connect(machine.socket(&root)).unwrap();
    write!(stream, "POST /v1/up HTTP/1.1\r\nHost: tay\r\n\r\n").unwrap();
    assert_eq!(newer.asked(), ["/v1/daemon", "/v1/up"]);
}
