//! Starting an environment's daemon: tay's own binary run as `tay daemon`,
//! in a session of its own, so that neither the terminal nor the shell that
//! ran the command can end it, with its log as its standard error. It says
//! whether it is ready by one line on its standard output, which the
//! command reads through a pipe rather than guessing with a sleep.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::control::daemon;
use crate::disk::{DiskError, Environment};

/// How long a daemon has to say it is ready.
pub const START: Duration = Duration::from_secs(10);

/// What a started daemon said.
pub enum Started {
    /// It listens.
    Ready,
    /// Another daemon held the environment's lock, so it exited.
    Held,
}

/// Why a daemon did not start.
#[derive(Debug, thiserror::Error)]
pub enum NotStarted {
    #[error("tay's own binary cannot be found to start the daemon: {0}")]
    NoBinary(std::io::Error),
    #[error("the daemon cannot be started: {0}")]
    Spawn(std::io::Error),
    #[error("the daemon cannot run: {why}")]
    CannotRun { why: String },
    #[error("the daemon ended without saying why; see `{}`", .0.display())]
    Silent(std::path::PathBuf),
    #[error("the daemon did not say it was ready within {} seconds; see `{}`", START.as_secs(), .0.display())]
    TooLate(std::path::PathBuf),
    #[error(transparent)]
    Disk(#[from] DiskError),
}

/// Starts the daemon of `environment` and waits until it says it runs.
pub fn start(environment: &Environment) -> Result<Started, NotStarted> {
    let binary = std::env::current_exe().map_err(NotStarted::NoBinary)?;
    let log_path = environment.layout.daemon_log();
    let log = daemon::open_log(&log_path)?;
    let mut command = Command::new(binary);
    command
        .arg("daemon")
        .arg("--root")
        .arg(environment.layout.root())
        // It holds no folder of the shell's, which could not be unmounted.
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(log);
    // SAFETY: the closure runs in the child between fork and exec, and
    // calls only `setsid`, which is safe to call there.
    unsafe {
        command.pre_exec(|| {
            rustix::process::setsid()?;
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(NotStarted::Spawn)?;
    let stdout = child
        .stdout
        .take()
        .expect("the daemon's standard output is piped");
    let (said, heard) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let read = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        // The command may have stopped waiting.
        let _ = said.send(read);
    });
    // The daemon is this command's child for as long as this command runs;
    // it is reaped if it ends meanwhile, and the system takes it over when
    // this command ends first.
    std::thread::spawn(move || child.wait());
    match heard.recv_timeout(START) {
        Ok(Ok(line)) => match line.trim_end() {
            "ready" => Ok(Started::Ready),
            "held" => Ok(Started::Held),
            "" => Err(NotStarted::Silent(log_path)),
            other => Err(NotStarted::CannotRun {
                why: other.strip_prefix("error: ").unwrap_or(other).to_string(),
            }),
        },
        Ok(Err(_)) => Err(NotStarted::Silent(log_path)),
        Err(_) => Err(NotStarted::TooLate(log_path)),
    }
}
