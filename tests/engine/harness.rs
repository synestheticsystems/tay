//! A machine of its own for each test: a home folder and the XDG folders in
//! a temporary folder, with the stacks a test brings up copied into it.
//! Every daemon started on it is stopped when it is dropped, also when the
//! test fails, and what its environments left in Docker removed, so nothing
//! a test starts outlives it.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

use crate::docker::Docker;
use crate::support::{Run, fixture};

pub struct Machine {
    folder: TempDir,
    /// Every stack copied onto it, which it takes down when it is dropped.
    stacks: std::cell::RefCell<Vec<PathBuf>>,
    /// Docker, for a test that needs it: dropped after the stacks are taken
    /// down, it removes whatever of their environments is left in it.
    docker: Option<Docker>,
}

impl Machine {
    pub fn new() -> Machine {
        let folder = tempfile::tempdir().expect("a temporary folder");
        for sub in ["home", "state", "data", "run", "stacks"] {
            fs::create_dir(folder.path().join(sub)).unwrap();
        }
        Machine {
            folder,
            stacks: std::cell::RefCell::default(),
            docker: None,
        }
    }

    /// A machine for a test that needs Docker, which it holds until the
    /// test ends.
    pub fn with_docker() -> Machine {
        let mut machine = Machine::new();
        machine.docker = Some(Docker::claim(&machine.path("state/tay/environments.json")));
        // The `docker` command a build runs finds its builder among its
        // plugins, in the home folder, which is the machine's own: they are
        // linked there, and whatever a build keeps stays there.
        if let Some(plugins) = crate::docker::plugins() {
            let docker = machine.path("home/.docker");
            fs::create_dir_all(&docker).unwrap();
            std::os::unix::fs::symlink(plugins, docker.join("cli-plugins")).unwrap();
        }
        machine
    }

    pub fn path(&self, sub: &str) -> PathBuf {
        self.folder.path().join(sub)
    }

    /// A copy of the fixture stack `name`, in a folder of that name.
    pub fn stack(&self, name: &str) -> PathBuf {
        self.stack_as(name, name)
    }

    /// A copy of the fixture stack `name`, in a folder `folder` under the
    /// machine's folder of stacks.
    pub fn stack_as(&self, name: &str, folder: &str) -> PathBuf {
        let to = self.path("stacks").join(folder);
        copy(&fixture(name), &to);
        let root = to.canonicalize().unwrap();
        self.stacks.borrow_mut().push(root.clone());
        root
    }

    /// Takes in a stack this test made on its own, so that it is taken
    /// down when the machine is dropped.
    pub fn adopt(&self, root: &Path) {
        self.stacks.borrow_mut().push(root.to_path_buf());
    }

    /// The command `tay ARGS`, run in `dir`, on this machine: its home and
    /// XDG folders are this machine's, and it reads nothing from a terminal.
    pub fn command(&self, dir: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tay"));
        command
            .args(args)
            .current_dir(dir)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.path("home"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_RUNTIME_DIR", self.path("run"))
            .stdin(Stdio::null());
        if let Some(host) = crate::docker::host() {
            command.env("DOCKER_HOST", host);
        }
        command
    }

    pub fn run(&self, mut command: Command) -> Run {
        let output = command.output().expect("the tay binary runs");
        Run {
            code: output.status.code().expect("tay exits with a code"),
            stdout: String::from_utf8(output.stdout).expect("output is UTF-8"),
            stderr: String::from_utf8(output.stderr).expect("errors are UTF-8"),
        }
    }

    pub fn tay(&self, dir: &Path, args: &[&str]) -> Run {
        self.run(self.command(dir, args))
    }

    /// `tay COMMAND --root ROOT ARGS`.
    pub fn at(&self, root: &Path, command: &str, args: &[&str]) -> Run {
        let mut all = vec![command, "--root", root.to_str().unwrap()];
        all.extend(args);
        self.tay(root, &all)
    }

    /// `tay up` at `root`, started, with its standard output read a line at
    /// a time.
    pub fn spawn_up(&self, root: &Path, args: &[&str]) -> Following {
        let mut all = vec!["up", "--root", root.to_str().unwrap()];
        all.extend(args);
        let mut command = self.command(root, &all);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().expect("the tay binary runs");
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Following {
            child,
            stdout,
            read: String::new(),
        }
    }

    /// `tay status --json` at `root` once the change under way has settled:
    /// `tay up` returns as soon as one unit it was asked for can no longer
    /// become ready, and the daemon goes on with the units that can.
    pub fn settled(&self, root: &Path) -> Value {
        let mut status = Value::Null;
        let settled = eventually(Duration::from_secs(20), || {
            status = self.at(root, "status", &["--json"]).json();
            status["units"].as_array().unwrap().iter().all(|unit| {
                unit["state"] != "starting"
                    && (unit["state"] != "waiting"
                        || unit["detail"].as_str().unwrap().starts_with("blocked"))
            })
        });
        assert!(settled, "the change settles: {status}");
        status
    }

    /// The units of the environment at `root`, in the order `tay status`
    /// gives them, which is the order they start in.
    pub fn state_order(&self, root: &Path) -> Vec<String> {
        self.at(root, "status", &["--json"]).json()["units"]
            .as_array()
            .unwrap()
            .iter()
            .map(|unit| unit["unit"].as_str().unwrap().to_string())
            .collect()
    }

    /// Waits until `tay status --json` at `root` gives `unit` a state and
    /// detail, as `state detail`, for which `wanted` holds, for as long as
    /// `limit`; the last one it gave.
    pub fn until_state(
        &self,
        root: &Path,
        unit: &str,
        limit: Duration,
        mut wanted: impl FnMut(&str) -> bool,
    ) -> String {
        let mut last = String::new();
        let found = eventually(limit, || {
            last = Machine::state(&self.at(root, "status", &["--json"]).json(), unit);
            wanted(&last)
        });
        assert!(
            found,
            "`{unit}` never came to the state wanted; it is `{last}`"
        );
        last
    }

    /// The state and detail of `unit` in a status, as `state detail`.
    pub fn state(status: &Value, unit: &str) -> String {
        let unit = status["units"]
            .as_array()
            .unwrap()
            .iter()
            .find(|status| status["unit"] == unit)
            .unwrap_or_else(|| panic!("`{unit}` is in {status}"));
        format!(
            "{} {}",
            unit["state"].as_str().unwrap(),
            unit["detail"].as_str().unwrap()
        )
    }

    /// The daemon of the environment at `root`, as `tay ls` reports it.
    pub fn daemon(&self, root: &Path) -> Option<Value> {
        let ls = self.tay(root, &["ls", "--json"]);
        ls.json()["environments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|environment| environment["folder"] == root.to_str().unwrap())
            .and_then(|environment| {
                let daemon = &environment["daemon"];
                (!daemon.is_null()).then(|| daemon.clone())
            })
    }

    pub fn daemon_pid(&self, root: &Path) -> Option<i32> {
        self.daemon(root)
            .map(|daemon| daemon["pid"].as_i64().unwrap() as i32)
    }

    /// Kills the daemon of the environment at `root` with `SIGKILL`, and
    /// waits until it is gone.
    pub fn kill_daemon(&self, root: &Path) {
        let pid = self.daemon_pid(root).expect("a daemon answers");
        let process = rustix::process::Pid::from_raw(pid).unwrap();
        rustix::process::kill_process(process, rustix::process::Signal::KILL).unwrap();
        assert!(
            eventually(Duration::from_secs(5), || !alive(pid)),
            "the daemon {pid} is gone"
        );
    }

    /// The environment's id.
    pub fn id(&self, root: &Path) -> String {
        fs::read_to_string(root.join(".tay/id"))
            .unwrap()
            .trim()
            .to_string()
    }

    /// The socket of the environment at `root`'s daemon.
    pub fn socket(&self, root: &Path) -> PathBuf {
        self.path("run")
            .join("tay")
            .join(self.id(root))
            .join("daemon.sock")
    }

    /// Asks the daemon of the environment at `root` for `path` with the
    /// JSON `body`, as a command does, and gives all it answers, read until
    /// it closes the connection: for a change, once the change has said its
    /// last.
    pub fn ask_daemon(&self, root: &Path, path: &str, body: &str) -> String {
        use std::io::{Read, Write};
        let mut stream = std::os::unix::net::UnixStream::connect(self.socket(root)).unwrap();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nHost: tay\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    }

    /// The machine's list of environments.
    pub fn list(&self) -> Value {
        let text = fs::read_to_string(self.path("state/tay/environments.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    /// Every process of this machine: each whose command line names this
    /// machine's folder, as every daemon, keeper and fixture script does,
    /// and each in a process group one of those leads, as what a unit
    /// starts in turn is. A group the test itself is in is led by no such
    /// process.
    pub fn processes(&self) -> Vec<i32> {
        let folder = self.folder.path().to_str().unwrap().to_string();
        let all = processes();
        let groups: Vec<i32> = all
            .iter()
            .filter(|(pid, group, command)| pid == group && command.contains(&folder))
            .map(|(_, group, _)| *group)
            .collect();
        all.iter()
            .filter(|(_, group, command)| command.contains(&folder) || groups.contains(group))
            .map(|(pid, _, _)| *pid)
            .collect()
    }
}

/// The command line of the process `pid`, empty when there is none.
pub fn command_of(pid: i32) -> String {
    processes()
        .into_iter()
        .find(|(other, _, _)| *other == pid)
        .map(|(_, _, command)| command)
        .unwrap_or_default()
}

/// Every process on this machine: its number, its group and its command
/// line.
pub fn processes() -> Vec<(i32, i32, String)> {
    let ps = Command::new("ps")
        .args(["-ax", "-o", "pid=,pgid=,command="])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&ps.stdout)
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid = words.next()?.parse().ok()?;
            let group = words.next()?.parse().ok()?;
            Some((pid, group, words.collect::<Vec<_>>().join(" ")))
        })
        .collect()
}

impl Drop for Machine {
    /// Takes down every environment whose daemon runs, and then kills any
    /// process of this machine that is still alive.
    fn drop(&mut self) {
        for root in self.stacks.take() {
            if root.join(".tay").exists() {
                let mut down = self.command(&root, &["down", "--root", root.to_str().unwrap()]);
                down.stdout(Stdio::null()).stderr(Stdio::null());
                if let Ok(mut child) = down.spawn() {
                    wait_for(&mut child, Duration::from_secs(10));
                }
            }
        }
        let until = Instant::now() + Duration::from_secs(10);
        while !self.processes().is_empty() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        // A test may leave processes behind on purpose, or fail with some
        // running: none outlives it.
        let until = Instant::now() + Duration::from_secs(5);
        while !self.processes().is_empty() && Instant::now() < until {
            for pid in self.processes() {
                if let Some(pid) = rustix::process::Pid::from_raw(pid) {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // What is left in Docker is found by the machine's list of
        // environments, so it goes before the machine's folder does.
        drop(self.docker.take());
    }
}

/// Waits for `child` to end, killing it if it has not within `limit`.
fn wait_for(child: &mut Child, limit: Duration) {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// A `tay up` under way, read as it prints.
pub struct Following {
    child: Child,
    stdout: BufReader<ChildStdout>,
    /// What it has printed so far.
    pub read: String,
}

impl Following {
    /// Reads what it prints until a line contains `text`.
    pub fn until(&mut self, text: &str) {
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).unwrap();
            assert!(
                read > 0,
                "`tay up` ended before it printed `{text}`:\n{}",
                self.read
            );
            self.read.push_str(&line);
            if line.contains(text) {
                return;
            }
        }
    }

    /// Waits for it to end, and gives all it printed.
    pub fn finish(mut self) -> Run {
        let mut rest = String::new();
        std::io::Read::read_to_string(&mut self.stdout, &mut rest).unwrap();
        self.read.push_str(&rest);
        let output = self.child.wait_with_output().unwrap();
        Run {
            code: output.status.code().expect("tay exits with a code"),
            stdout: self.read,
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }
}

/// Copies the folder `from` to `to`, everything in it included.
pub fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Waits until `done` holds, for as long as `limit`; whether it did.
pub fn eventually(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    done()
}

/// The port numbers the environment at `root` keeps, by unit.
pub fn kept_ports(root: &Path) -> serde_json::Map<String, Value> {
    let text = fs::read_to_string(root.join(".tay/state/ports.json")).unwrap();
    let record: Value = serde_json::from_str(&text).unwrap();
    record["units"].as_object().unwrap().clone()
}

/// What the record of `unit`, a unit that runs on the host, says now.
pub fn record(root: &Path, unit: &str) -> Value {
    let path = root.join(".tay/state/units").join(unit).join("record.json");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).unwrap()
}

/// The process the keeper of `unit` started, as its record says.
pub fn child(root: &Path, unit: &str) -> i32 {
    record(root, unit)["child"]["pid"]
        .as_i64()
        .expect("the record names a child") as i32
}

/// All that `unit`'s log holds now, the file being written.
pub fn log(root: &Path, unit: &str) -> String {
    fs::read_to_string(root.join(".tay/logs").join(unit).join("output.log")).unwrap_or_default()
}

/// Whether a process with this number exists.
pub fn alive(pid: i32) -> bool {
    rustix::process::Pid::from_raw(pid)
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}
