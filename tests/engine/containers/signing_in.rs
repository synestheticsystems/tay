//! An image in a registry that wants a sign-in: pulled with the one the
//! account's Docker configuration holds for that registry, and refused
//! without one, the unit's reason naming the registry, the configuration
//! looked in and the `docker login` that gives one. The registry is the
//! test's own, on a loopback port, which Docker reaches without TLS by its
//! own rule. It, the image pulled from it and everything else it made go
//! when the test ends, also when it fails; the `docker` command it is driven
//! by reads a configuration of the test's own, never the account's.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use tempfile::TempDir;

use super::container;
use crate::docker;
use crate::harness::{Machine, eventually};

/// The registry's own image.
const IMAGE: &str = "registry:2";

/// The image the test pushes to it, from one the other tests use.
const PUSHED: &str = "tay-test/private:1";
const FROM: &str = "busybox:1.36";

/// The test's sign-in, as `htpasswd -nbB` writes it for the registry, and
/// as `docker login` writes it for the `docker` command: `tay` and a
/// password whose three `?`s put a `/` into the standard base64 of what
/// carries the sign-in to Docker, wherever it falls, and so make sure that
/// what Docker is sent is in the alphabet it reads.
const PASSWORDS: &str = "tay:$2y$05$ZFWqY5klRdT0czM1Z3nRmunhnB64anR5taDwRevgPDpVOkE44KY0i\n";
const AUTH: &str = "dGF5OnRheS10ZXN0LT8/Py1wYXNzd29yZA==";

#[test]
fn a_private_image_is_pulled_with_the_accounts_sign_in_and_refused_without_one() {
    let registry = Registry::start();
    // Declared after the registry, so dropped before it: its environments
    // are gone before the registry and the image they pulled are.
    let machine = Machine::with_docker();
    let image = registry.pushed();
    let set = format!("settings.box={image}");

    let empty = machine.path("no-sign-in");
    fs::create_dir(&empty).unwrap();
    let without = machine.stack_as("containers", "without");
    let up = up_with(&machine, &without, &empty, &set);
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    let failed = up.last_change("tagged");
    let address = registry.address();
    let account = format!("uid {}", rustix::process::getuid().as_raw());
    assert!(
        failed.starts_with(&format!("failed Docker could not pull `{image}`: ")),
        "{failed}"
    );
    assert!(
        failed.contains(&format!(
            ". No sign-in to `{address}` went with it: the Docker configuration of the account tay runs as, {account}, has none for it in `{}`; `docker login {address}` as {account} gives it one",
            empty.join("config.json").display()
        )),
        "{failed}"
    );

    let with = machine.stack_as("containers", "with");
    let up = up_with(&machine, &with, &registry.signed_in(), &set);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(
        up.changes()
            .contains(&format!("tagged starting attempt 1, pulling `{image}`")),
        "{}",
        up.stdout
    );
    assert_eq!(
        container(&machine, &with, "tagged").unwrap()["Config"]["Image"],
        image.as_str()
    );
}

/// `tay up tagged --set SET` at `root`, its daemon reading the Docker
/// configuration in `config`.
fn up_with(machine: &Machine, root: &Path, config: &Path, set: &str) -> crate::support::Run {
    let root_text = root.to_str().unwrap();
    let mut up = machine.command(root, &["up", "--root", root_text, "tagged", "--set", set]);
    up.env("DOCKER_CONFIG", config);
    machine.run(up)
}

/// A registry of the test's own that wants a sign-in, and the `docker`
/// command's configuration that holds it, in a folder of the test's own.
struct Registry {
    folder: TempDir,
    port: u16,
    /// Its container's name.
    name: String,
    /// Whether its image was pulled for it, and so goes with it.
    pulled: bool,
}

impl Registry {
    /// The registry, running and answering, with [`PUSHED`] pushed to it
    /// and then taken away from Docker's own images, so that a pull is
    /// what brings it back.
    fn start() -> Registry {
        let folder = tempfile::tempdir().expect("a temporary folder");
        let port = free_port();
        let mut registry = Registry {
            name: format!("tay-test-registry-{port}"),
            port,
            pulled: false,
            folder,
        };
        let auth = registry.folder.path().join("auth");
        fs::create_dir(&auth).unwrap();
        fs::write(auth.join("htpasswd"), PASSWORDS).unwrap();
        let signed_in = registry.signed_in();
        fs::create_dir(&signed_in).unwrap();
        fs::write(
            signed_in.join("config.json"),
            format!(
                r#"{{"auths":{{"{}":{{"auth":"{AUTH}"}}}}}}"#,
                registry.address()
            ),
        )
        .unwrap();
        if !registry
            .docker(&["image", "inspect", IMAGE])
            .status
            .success()
        {
            registry.pulled = true;
            registry.must(&["pull", "--quiet", IMAGE]);
        }
        let volume = format!("{}:/auth:ro", auth.display());
        let published = format!("127.0.0.1:{port}:5000");
        registry.must(&[
            "run",
            "--detach",
            "--name",
            &registry.name,
            "--publish",
            &published,
            "--volume",
            &volume,
            "--env",
            "REGISTRY_AUTH=htpasswd",
            "--env",
            "REGISTRY_AUTH_HTPASSWD_REALM=tay-test",
            "--env",
            "REGISTRY_AUTH_HTPASSWD_PATH=/auth/htpasswd",
            IMAGE,
        ]);
        assert!(
            eventually(Duration::from_secs(60), || wants_a_sign_in(port)),
            "the registry answers on port {port}"
        );
        if !registry
            .docker(&["image", "inspect", FROM])
            .status
            .success()
        {
            registry.must(&["pull", "--quiet", FROM]);
        }
        let pushed = registry.pushed();
        registry.must(&["tag", FROM, &pushed]);
        registry.must(&["push", "--quiet", &pushed]);
        registry.must(&["rmi", &pushed]);
        registry
    }

    /// Where Docker reaches it.
    fn address(&self) -> String {
        format!("localhost:{}", self.port)
    }

    /// The image pushed to it, by its whole name.
    fn pushed(&self) -> String {
        format!("{}/{PUSHED}", self.address())
    }

    /// A folder holding a Docker configuration with the sign-in to it, as
    /// `docker login` writes one.
    fn signed_in(&self) -> PathBuf {
        self.folder.path().join("signed-in")
    }

    /// `docker ARGS`, reading the test's own configuration, which holds the
    /// sign-in, and nothing of the account's.
    fn docker(&self, args: &[&str]) -> Output {
        let mut command = Command::new("docker");
        command
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.folder.path())
            .env("DOCKER_CONFIG", self.signed_in())
            .env(
                "DOCKER_HOST",
                docker::host().expect("where Docker listens is known"),
            )
            .stdin(Stdio::null());
        command.output().expect("`docker` runs")
    }

    /// `docker ARGS`, which must succeed.
    fn must(&self, args: &[&str]) {
        let output = self.docker(args);
        assert!(
            output.status.success(),
            "`docker {}` fails: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl Drop for Registry {
    /// Removes the registry with what it keeps, the image pulled from it,
    /// and its own image if it was pulled for it.
    fn drop(&mut self) {
        self.docker(&["rm", "--force", "--volumes", &self.name]);
        self.docker(&["rmi", "--force", &self.pushed()]);
        if self.pulled {
            self.docker(&["rmi", IMAGE]);
        }
    }
}

/// A loopback port in the range the tests may use that nothing listens on
/// now.
fn free_port() -> u16 {
    let first = std::process::id() % 10000;
    (0..10000)
        .map(|offset| (20000 + (first + offset) % 10000) as u16)
        .find(|port| TcpListener::bind(("127.0.0.1", *port)).is_ok())
        .expect("a free port")
}

/// Whether the registry on `port` answers, as one that wants a sign-in
/// does, with 401.
fn wants_a_sign_in(port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    if stream
        .write_all(b"GET /v2/ HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut answer = [0; 12];
    stream.read_exact(&mut answer).is_ok() && answer.ends_with(b" 401")
}
