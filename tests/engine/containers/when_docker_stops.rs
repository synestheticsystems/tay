//! When Docker itself stops, as it does when it is upgraded or restarted,
//! every container ends with it, and none of that is a unit's doing. While
//! Docker does not answer, each unit in a container waits for it and
//! nothing is counted against it; when it answers again, the daemon brings
//! the environment up, with no command, and says in its log and in
//! `tay status` when Docker stopped answering and when the environment was
//! brought up again. Host processes are not touched.
//!
//! The daemon reaches Docker here through a socket of the test's own that
//! passes everything on to this machine's Docker: stopping it is, for tay,
//! Docker stopping, as every connection ends at once and none is taken
//! until it is started again. Docker itself is never restarted, since
//! other work shares it; the test ends the environment's containers in it
//! as Docker would.

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::name;
use crate::docker::{self, docker};
use crate::harness::{Machine, child};
use crate::support::Run;

/// How long a test waits for the environment to settle as it should, on a
/// loaded machine.
const SETTLES: Duration = Duration::from_secs(90);

/// A socket that passes every connection on to Docker, until it is
/// stopped.
struct DockerThrough {
    path: PathBuf,
    docker: PathBuf,
    passing: Option<Passing>,
}

/// The socket while it passes connections on.
struct Passing {
    stopping: Arc<AtomicBool>,
    /// Both ends of each connection passed on.
    connections: Arc<Mutex<Vec<UnixStream>>>,
    accepting: JoinHandle<()>,
}

impl DockerThrough {
    /// A socket at `path` passing every connection on to this machine's
    /// Docker.
    fn start(path: PathBuf) -> DockerThrough {
        let host = docker::host().expect("where Docker listens is known");
        let docker = PathBuf::from(
            host.strip_prefix("unix://")
                .unwrap_or_else(|| panic!("Docker listens on a local socket, not `{host}`")),
        );
        let mut through = DockerThrough {
            path,
            docker,
            passing: None,
        };
        through.answer();
        through
    }

    /// `DOCKER_HOST` for a daemon that is to reach Docker through it.
    fn host(&self) -> String {
        format!("unix://{}", self.path.display())
    }

    /// Passes connections on again.
    fn answer(&mut self) {
        let listener = UnixListener::bind(&self.path).unwrap();
        let stopping = Arc::new(AtomicBool::new(false));
        let connections = Arc::new(Mutex::new(Vec::new()));
        let docker = self.docker.clone();
        let accepting = {
            let stopping = Arc::clone(&stopping);
            let connections = Arc::clone(&connections);
            std::thread::spawn(move || {
                for client in listener.incoming() {
                    if stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(client) = client else { continue };
                    let Ok(server) = UnixStream::connect(&docker) else {
                        continue;
                    };
                    let mut held = connections.lock().unwrap();
                    held.push(client.try_clone().unwrap());
                    held.push(server.try_clone().unwrap());
                    pass(client.try_clone().unwrap(), server.try_clone().unwrap());
                    pass(server, client);
                }
            })
        };
        self.passing = Some(Passing {
            stopping,
            connections,
            accepting,
        });
    }

    /// Stops, as Docker does: every connection ends at once, and none is
    /// taken until it answers again.
    fn stop(&mut self) {
        let passing = self.passing.take().expect("it passes connections on");
        passing.stopping.store(true, Ordering::SeqCst);
        // The thread that waits for a connection is woken by one.
        let _ = UnixStream::connect(&self.path);
        passing.accepting.join().unwrap();
        std::fs::remove_file(&self.path).unwrap();
        for connection in passing.connections.lock().unwrap().drain(..) {
            let _ = connection.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for DockerThrough {
    fn drop(&mut self) {
        if self.passing.is_some() {
            self.stop();
        }
    }
}

/// Copies what `from` says to `to` until it ends, then ends `to` too.
fn pass(mut from: UnixStream, mut to: UnixStream) {
    std::thread::spawn(move || {
        let mut buffer = [0; 64 * 1024];
        while let Ok(read) = from.read(&mut buffer) {
            if read == 0 || to.write_all(&buffer[..read]).is_err() {
                break;
            }
        }
        let _ = to.shutdown(Shutdown::Write);
    });
}

/// `tay COMMAND --root ROOT ARGS` at `root`, reaching Docker through
/// `through`, as the daemon it starts does.
fn through(machine: &Machine, through: &DockerThrough, root: &Path, args: &[&str]) -> Run {
    let mut all = vec![args[0], "--root", root.to_str().unwrap()];
    all.extend(&args[1..]);
    let mut command = machine.command(root, &all);
    command.env("DOCKER_HOST", through.host());
    machine.run(command)
}

/// Ends the containers of `units` in the environment at `root` as Docker
/// does as it stops: told to stop, and killed after a moment.
fn end_as_docker_does(machine: &Machine, root: &Path, units: &[&str]) {
    let mut args = vec!["stop", "-t", "2"];
    let names: Vec<String> = units.iter().map(|unit| name(machine, root, unit)).collect();
    args.extend(names.iter().map(String::as_str));
    docker(&args);
}

/// Waits until `unit` waits for Docker, as tay says it.
fn waits_for_docker(machine: &Machine, root: &Path, unit: &str) {
    machine.until_state(root, unit, SETTLES, |state| {
        state.starts_with("waiting for Docker, which stopped answering at ")
    });
}

/// Waits until each of `units` is ready, started afresh.
fn ready_afresh(machine: &Machine, root: &Path, units: &[&str]) {
    for unit in units {
        machine.until_state(root, unit, SETTLES, |state| {
            state.starts_with("ready attempt 1,")
        });
    }
}

/// How many times the keeper has started, as its volume counts them.
fn keeper_starts(machine: &Machine, root: &Path) -> usize {
    let starts = docker(&[
        "exec",
        &name(machine, root, "keeper"),
        "cat",
        "/data/starts",
    ]);
    starts.lines().count()
}

#[test]
fn docker_stopping_under_a_stack_that_is_up_brings_it_up_again_once_docker_answers() {
    let folder = tempfile::tempdir().unwrap();
    let mut docker_through = DockerThrough::start(folder.path().join("docker.sock"));
    let machine = Machine::with_docker();
    let root = machine.stack("docker-stops");
    let up = through(&machine, &docker_through, &root, &["up"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(keeper_starts(&machine, &root), 1);
    let local = child(&root, "local");
    docker_through.stop();
    end_as_docker_does(&machine, &root, &["polite", "stubborn", "keeper"]);
    for unit in ["polite", "stubborn", "keeper"] {
        waits_for_docker(&machine, &root, unit);
    }
    // A `tay up` while it does not answer cannot tell what of the
    // environment is left in Docker, and leaves each unit waiting.
    let up = through(&machine, &docker_through, &root, &["up"]);
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    for unit in ["polite", "stubborn", "keeper"] {
        waits_for_docker(&machine, &root, unit);
    }
    let status = machine.at(&root, "status", &[]);
    assert!(
        status.stdout.contains("  Docker stopped answering at 20")
            && status
                .stdout
                .contains("; the environment has not been brought up again since\n"),
        "{}",
        status.stdout
    );
    assert!(
        status
            .stdout
            .contains("  local     ready     attempt 1, running\n"),
        "{}",
        status.stdout
    );
    docker_through.answer();
    // With no command, each unit in a container is started afresh, the
    // volume holding what it held; the host process ran on.
    ready_afresh(&machine, &root, &["polite", "stubborn", "keeper"]);
    assert_eq!(keeper_starts(&machine, &root), 2);
    assert_eq!(child(&root, "local"), local);
    let status = machine.at(&root, "status", &["--json"]).json();
    let outage = &status["environment"]["outage"];
    assert_eq!(outage["of"], "Docker");
    let (stopped, again) = (
        outage["stopped_answering"].as_u64().unwrap(),
        outage["brought_up_again"].as_u64().unwrap(),
    );
    assert!(stopped <= again, "{outage}");
    let log = std::fs::read_to_string(root.join(".tay/logs/tay/daemon.log")).unwrap();
    for said in [
        "  Docker stopped answering at 20",
        "  Docker answers again, so the environment is brought up again\n",
    ] {
        assert!(log.contains(said), "`{said}`: {log}");
    }
}

#[test]
fn what_ended_moments_before_docker_stopped_answering_is_not_held_against_it() {
    let folder = tempfile::tempdir().unwrap();
    let mut docker_through = DockerThrough::start(folder.path().join("docker.sock"));
    let machine = Machine::with_docker();
    let root = machine.stack("docker-stops");
    let up = through(&machine, &docker_through, &root, &["up"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    // Docker ends its containers as it begins to stop, while it still
    // answers, and tay sees them end: one that ends with 0 is not started
    // again, since its `restart` is `failure`.
    end_as_docker_does(&machine, &root, &["polite", "stubborn"]);
    machine.until_state(&root, "polite", SETTLES, |state| {
        state.starts_with("failed attempt 1 exited with 0")
    });
    docker_through.stop();
    waits_for_docker(&machine, &root, "polite");
    docker_through.answer();
    ready_afresh(&machine, &root, &["polite", "stubborn", "keeper"]);
}

#[test]
fn a_stack_with_no_unit_in_a_container_asks_nothing_of_docker() {
    let machine = Machine::new();
    let root = machine.stack("services");
    // Docker cannot be reached here: one that is asked does not answer.
    let mut command = machine.command(&root, &["up", "--root", root.to_str().unwrap()]);
    command.env(
        "DOCKER_HOST",
        format!("unix://{}", machine.path("nothing.sock").display()),
    );
    let up = machine.run(command);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let status = machine.at(&root, "status", &["--json"]).json();
    assert_eq!(status["environment"]["outage"], serde_json::Value::Null);
    let log = std::fs::read_to_string(root.join(".tay/logs/tay/daemon.log")).unwrap();
    assert!(!log.contains("stopped answering"), "{log}");
}
