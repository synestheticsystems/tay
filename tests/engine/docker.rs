//! Docker for the tests that need it, which is shared with other work on
//! this machine. A test that needs Docker says so with [`Docker::claim`],
//! and fails at once, saying why, when Docker cannot be reached: none passes
//! by skipping. Only a few such tests run at once; every container their
//! environments create is held to a small memory limit as soon as it is
//! created; and once a test ends, also when it fails, every container,
//! volume and network that carries the label of one of its environments is
//! removed. Nothing without such a label is ever touched here.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Condvar, Mutex, OnceLock};

/// How many tests may hold Docker at once.
const AT_ONCE: usize = 3;

/// The memory every test container is held to, unless its test says
/// otherwise.
pub const MEMORY: &str = "64m";

static CLAIMED: Mutex<usize> = Mutex::new(0);
static FREED: Condvar = Condvar::new();

/// Where the tests' Docker listens, as this machine's own configuration
/// says: what each `tay` a test runs is pointed at, since its home folder is
/// a temporary one.
pub fn host() -> Option<&'static str> {
    static HOST: OnceLock<Option<String>> = OnceLock::new();
    HOST.get_or_init(|| {
        if let Ok(host) = std::env::var("DOCKER_HOST")
            && !host.is_empty()
        {
            return Some(host);
        }
        let inspected = Command::new("docker")
            .args([
                "context",
                "inspect",
                "--format",
                "{{.Endpoints.docker.Host}}",
            ])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let host = String::from_utf8(inspected.stdout).ok()?.trim().to_string();
        (inspected.status.success() && !host.is_empty()).then_some(host)
    })
    .as_deref()
}

/// The folder of the `docker` command's plugins in this machine's own
/// configuration, when there is one.
pub fn plugins() -> Option<std::path::PathBuf> {
    let config = match std::env::var_os("DOCKER_CONFIG") {
        Some(config) => std::path::PathBuf::from(config),
        None => std::path::PathBuf::from(std::env::var_os("HOME")?).join(".docker"),
    };
    let plugins = config.join("cli-plugins");
    plugins.is_dir().then_some(plugins)
}

/// Docker, held by one test until it is dropped.
pub struct Docker {
    /// The machine's list of environments, whose ids say what is the
    /// test's.
    list: std::path::PathBuf,
    /// Follows what Docker creates, to hold each of the test's containers
    /// to its memory limit.
    events: Child,
}

impl Docker {
    /// Docker for a test whose machine keeps its list of environments at
    /// `list`, once a place among the tests that hold it is free. Docker
    /// that cannot be reached fails the test, saying so.
    pub fn claim(list: &Path) -> Docker {
        let reached = Command::new("docker")
            .args(["version", "--format", "{{.Server.Version}}"])
            .output();
        match reached {
            Ok(output) if output.status.success() => {}
            Ok(output) => panic!(
                "this test needs Docker, and it cannot be reached: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Err(error) => panic!("this test needs Docker, and `docker` cannot be run: {error}"),
        }
        assert!(
            host().is_some(),
            "this test needs Docker, and where it listens cannot be told"
        );
        let mut claimed = CLAIMED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *claimed >= AT_ONCE {
            claimed = FREED
                .wait(claimed)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *claimed += 1;
        drop(claimed);
        let mut events = Command::new("docker")
            .args([
                "events",
                "--filter",
                "type=container",
                "--filter",
                "event=create",
                "--filter",
                "label=tay.environment",
                "--format",
                "{{.Actor.ID}} {{index .Actor.Attributes \"tay.environment\"}}",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("`docker events` runs");
        let created = BufReader::new(events.stdout.take().unwrap());
        let list = list.to_path_buf();
        let ours = list.clone();
        std::thread::spawn(move || {
            for line in created.lines() {
                let Ok(line) = line else { return };
                let Some((container, environment)) = line.split_once(' ') else {
                    continue;
                };
                if ids(&ours).iter().any(|id| id == environment) {
                    docker(&[
                        "update",
                        "--memory",
                        MEMORY,
                        "--memory-swap",
                        MEMORY,
                        container,
                    ]);
                }
            }
        });
        Docker { list, events }
    }
}

impl Drop for Docker {
    /// Removes every container, volume and network of the test's
    /// environments, and frees the test's place.
    fn drop(&mut self) {
        let _ = self.events.kill();
        let _ = self.events.wait();
        for id in ids(&self.list) {
            remove_all_of(&id);
        }
        let mut claimed = CLAIMED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *claimed -= 1;
        FREED.notify_one();
    }
}

/// Removes every container, volume and network that carries the label of
/// the environment `id`, and nothing else.
pub fn remove_all_of(id: &str) {
    let label = format!("label=tay.environment={id}");
    for (list, remove) in [
        (vec!["ps", "-aq", "--filter", &label], vec!["rm", "-f"]),
        (
            vec!["volume", "ls", "-q", "--filter", &label],
            vec!["volume", "rm", "-f"],
        ),
        (
            vec!["network", "ls", "-q", "--filter", &label],
            vec!["network", "rm"],
        ),
    ] {
        let listed = docker(&list);
        let names: Vec<&str> = listed.split_whitespace().collect();
        if names.is_empty() {
            continue;
        }
        let mut args = remove.clone();
        args.extend(names);
        docker(&args);
    }
}

/// The ids of the environments in the machine's list at `list`.
fn ids(list: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(list) else {
        return Vec::new();
    };
    let Ok(list) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    list["environments"]
        .as_array()
        .map(|environments| {
            environments
                .iter()
                .filter_map(|environment| environment["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// What `docker ARGS` prints, run with this machine's own configuration.
pub fn docker(args: &[&str]) -> String {
    let output = Command::new("docker")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .expect("`docker` runs");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// What `docker inspect` says of the object `name`, of any kind, as JSON;
/// `None` when there is none.
pub fn inspect(name: &str) -> Option<serde_json::Value> {
    let output = Command::new("docker")
        .args(["inspect", name])
        .stderr(Stdio::null())
        .output()
        .expect("`docker` runs");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    value.as_array()?.first().cloned()
}
