//! An environment's daemon, `tay daemon`: one per environment, started
//! detached by the first command that needs it. It owns the changes to the
//! environment, listens on a Unix socket in a short folder named by the
//! environment's id, and lives for as long as it holds `.tay/daemon.lock`.
//!
//! It tells the command that started it whether it is ready by one line on
//! its standard output, which that command reads through a pipe: `ready`
//! once it listens, `held` when another daemon holds the lock, or `error:`
//! and why it cannot run. Everything else it says goes to its log, which is
//! its standard error.

mod change;
mod down;
mod routes;
mod serve;
mod up;

use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tokio::net::UnixListener;
use tokio::sync::watch;

use crate::control::clock;
use crate::control::protocol::{self, Event, UpRequest};
use crate::definition::read::THIS_TAY;
use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Sources;
use crate::disk::{DiskError, Environment, Identity, Intent, Layout, Lock, Machine};
use crate::engine::{self, UnitState, Units};
use crate::module::Values;
use crate::plan::{self, Plan, PlannedUnit};
use crate::print;

use change::Change;

/// What the daemon holds while it runs.
pub struct Daemon {
    environment: Arc<Environment>,
    about: protocol::About,
    units: Mutex<Units>,
    /// The change under way, if any.
    change: Mutex<Option<Arc<Change>>>,
    /// Why the daemon is to exit, once it is.
    exit: watch::Sender<Option<String>>,
    /// Whether it has begun to stop everything because the environment's
    /// folder has gone.
    going: AtomicBool,
}

/// Why the daemon cannot start.
#[derive(Debug, thiserror::Error)]
enum CannotStart {
    #[error(transparent)]
    Disk(#[from] DiskError),
    #[error("this folder has no id of its own: no `tay up` has been run in it")]
    NotIdentified,
    #[error("`{}` cannot be listened on: {source}", path.display())]
    Socket {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("tay cannot start the daemon's runtime: {0}")]
    Runtime(std::io::Error),
}

/// Runs the daemon of the environment whose stack's root is `root`, until
/// it is taken down, asked to exit, or its folder or socket disappears.
/// Gives whether it could run; one that finds another daemon running
/// leaves it to that one, which is as it should be.
pub fn run(root: &Path) -> bool {
    let layout = Layout::of(root);
    let lock = match Lock::try_take(&layout.daemon_lock()) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            ready_line("held");
            return true;
        }
        Err(error) => return cannot_start(error.into()),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return cannot_start(CannotStart::Runtime(error)),
    };
    runtime.block_on(async {
        let (daemon, listener, socket) = match start(layout).await {
            Ok(started) => started,
            Err(why) => return cannot_start(why),
        };
        ready_line("ready");
        serve::serve(&daemon, listener, &lock, &socket).await;
        // The socket is removed while the lock is still held, so no other
        // daemon has made one there yet.
        if socket.is_ours() {
            let _ = std::fs::remove_file(&socket.path);
        }
        true
    })
}

/// The socket the daemon listens on, and which file it is, so that the
/// daemon can tell when it has been removed or replaced.
struct Socket {
    path: PathBuf,
    identity: (u64, u64),
}

impl Socket {
    fn is_ours(&self) -> bool {
        std::fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.identity)
    }
}

async fn start(layout: Layout) -> Result<(Arc<Daemon>, UnixListener, Socket), CannotStart> {
    let machine = Machine::from_env()?;
    let Identity::Own(environment) = Environment::identify(&layout, &machine)? else {
        return Err(CannotStart::NotIdentified);
    };
    let path = environment.socket();
    machine.make_socket_folder(environment.id.as_str())?;
    // The daemon holds the lock, so a socket file there is stale.
    let _ = std::fs::remove_file(&path);
    let listen_failed = |source| CannotStart::Socket {
        path: path.clone(),
        source,
    };
    let listener = UnixListener::bind(&path).map_err(listen_failed)?;
    // Who may control the environment is decided by the socket's
    // permissions alone.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(listen_failed)?;
    let metadata = std::fs::symlink_metadata(&path).map_err(listen_failed)?;
    let socket = Socket {
        path,
        identity: (metadata.dev(), metadata.ino()),
    };
    let about = protocol::About {
        tay: THIS_TAY.to_string(),
        pid: std::process::id(),
        root: environment.layout.root().to_path_buf(),
        id: environment.id.to_string(),
        started: clock::now(),
    };
    log(&format!(
        "started: tay {THIS_TAY}, process {}, for {}",
        about.pid,
        about.root.display()
    ));
    let environment = Arc::new(environment);
    let units = found_again(&environment);
    let daemon = Arc::new(Daemon {
        environment,
        about,
        units: Mutex::new(units),
        change: Mutex::new(None),
        exit: watch::Sender::new(None),
        going: AtomicBool::new(false),
    });
    Ok((daemon, listener, socket))
}

/// What the daemon knows of the units when it starts: those `tay plan`
/// would list, each found as it is when the environment is meant to be up,
/// and stopped when it is not. Choices `tay up` was given are not kept, so
/// they are not what this plan is made with.
fn found_again(environment: &Environment) -> Units {
    let mut units = Units::default();
    let request = UpRequest {
        units: Vec::new(),
        profile: None,
        sets: Vec::new(),
        home: std::env::var_os("HOME").map(PathBuf::from),
    };
    let planned = match planned(environment.layout.root(), &request) {
        Ok(plan) => plan,
        Err(unplanned) => {
            log(&format!(
                "the definitions do not plan, so no unit is known yet:\n{}",
                unplanned.lines.join("\n")
            ));
            return units;
        }
    };
    let plan: Vec<Arc<PlannedUnit>> = planned.units.into_iter().map(Arc::new).collect();
    units.take_in(&plan);
    match Intent::read(&environment.layout) {
        Ok(Intent::Up) => {
            for found in engine::find_again(&plan, environment) {
                log_state(&found.unit, &found.state);
                units.record(&found.unit, found.state, found.values);
            }
        }
        Ok(Intent::Down) => {}
        Err(error) => log(&format!("{error}; every unit is taken to be stopped")),
    }
    units
}

/// The definitions did not plan: each error as `tay plan` prints it.
struct Unplanned {
    lines: Vec<String>,
    too_old: bool,
}

/// Plans the stack at `root` as `tay plan` would with what `request` asks.
fn planned(root: &Path, request: &UpRequest) -> Result<Plan, Unplanned> {
    let options = request.options();
    let mut sources = Sources::default();
    let mut d = Diagnostics::default();
    let rendered = |errors: Vec<crate::diagnostic::Diagnostic>, sources: &Sources| {
        errors.iter().map(|error| error.render(sources)).collect()
    };
    match plan::plan(root, &options, &mut sources, &mut d) {
        plan::Outcome::Plan(plan) => Ok(plan),
        plan::Outcome::Errors => Err(Unplanned {
            lines: rendered(d.into_sorted(), &sources),
            too_old: false,
        }),
        plan::Outcome::TooOld(reasons) => Err(Unplanned {
            lines: rendered(reasons, &sources),
            too_old: true,
        }),
    }
}

/// Says to the command that started the daemon whether it runs. Nothing
/// more is written to standard output, which is that command's pipe.
fn ready_line(line: &str) {
    let mut out = std::io::stdout().lock();
    // A command that has stopped waiting for the line has gone; the daemon
    // carries on, or exits, as it would have.
    let _ = writeln!(out, "{line}").and_then(|()| out.flush());
}

fn cannot_start(why: CannotStart) -> bool {
    log(&format!("cannot start: {why}"));
    ready_line(&format!("error: {why}"));
    false
}

impl Daemon {
    /// Asks the daemon to exit, once the answers under way have finished.
    /// The stack is left as it is, and a change under way stops.
    pub fn exit(&self, why: &str) {
        if let Some(change) = self
            .change
            .lock()
            .expect("the change under way is not poisoned")
            .as_ref()
        {
            change.cancel("the daemon's exit");
        }
        self.exit.send_if_modified(|exit| {
            let first = exit.is_none();
            if first {
                *exit = Some(why.to_string());
            }
            first
        });
    }

    /// Records that `unit` took `state`, and tells `change`'s followers.
    fn changed(&self, change: &Change, unit: &str, state: UnitState, values: Option<Values>) {
        log_state(unit, &state);
        self.units
            .lock()
            .expect("the units are not poisoned")
            .record(unit, state.clone(), values);
        change.say(Event::State {
            unit: unit.to_string(),
            state: state.state,
            detail: state.detail,
            at: clock::now(),
        });
    }

    /// The change is over: what followers are told ends, and another
    /// change may begin.
    fn finish(&self, change: &Arc<Change>) {
        change.close();
        change.end();
        let mut under_way = self
            .change
            .lock()
            .expect("the change under way is not poisoned");
        if under_way
            .as_ref()
            .is_some_and(|other| Arc::ptr_eq(other, change))
        {
            *under_way = None;
        }
    }
}

/// Writes a line to the daemon's log, with its time.
fn log(line: &str) {
    print::err(&format!("{}  {line}\n", clock::utc(clock::now())));
}

fn log_state(unit: &str, state: &UnitState) {
    log(&format!(
        "{unit}: {} ({})",
        state.state.word(),
        state.detail
    ));
}
