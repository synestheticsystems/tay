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
mod restart;
mod routes;
mod serve;
mod up;

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tokio::net::UnixListener;
use tokio::sync::watch;

use crate::clock;
use crate::control::protocol::{self, Event, UpRequest};
use crate::definition::read::THIS_TAY;
use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Sources;
use crate::disk::{Asked, Counts, DiskError, Environment, Identity, Intent, Layout, Lock, Machine};
use crate::engine::{self, Supervisor, Told, UnitState};
use crate::module::Occasion;
use crate::plan::{self, Plan, PlannedUnit};
use crate::print;

use change::Change;

/// What the daemon holds while it runs.
pub struct Daemon {
    environment: Arc<Environment>,
    about: protocol::About,
    /// The units, and the tasks that look after those that run.
    supervisor: Arc<Supervisor>,
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
    // The command that starts the daemon gives it this log as its standard
    // error. It is set once, as the daemon starts.
    let _ = LOG.set(layout.daemon_log());
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
    let counts = Counts::read(&environment.layout).unwrap_or_else(|error| {
        log(&format!("{error}; attempts are counted afresh"));
        Counts::none(&environment.layout)
    });
    let environment = Arc::new(environment);
    let daemon = Arc::new_cyclic(|daemon: &Weak<Daemon>| {
        let daemon = Weak::clone(daemon);
        let told: Told = Box::new(move |unit, state| {
            log_state(unit, state);
            if let Some(daemon) = daemon.upgrade() {
                daemon.tell(unit, state);
            }
        });
        Daemon {
            supervisor: Arc::new(Supervisor::new(Arc::clone(&environment), counts, told)),
            environment,
            about,
            change: Mutex::new(None),
            exit: watch::Sender::new(None),
            going: AtomicBool::new(false),
        }
    });
    found_again(&daemon.supervisor).await;
    Ok((daemon, listener, socket))
}

/// Finds the units again as the daemon starts: those `tay plan` would
/// list, with what the environment remembers being asked for, each as it is
/// found, and each that runs that the plan does not have. What runs is
/// looked after again whether or not the environment is meant to be up,
/// and what is made is checked when it is.
async fn found_again(supervisor: &Arc<Supervisor>) {
    let layout = &supervisor.environment.layout;
    let asked = match Asked::read(layout) {
        Ok(asked) => asked,
        Err(error) => {
            log(&format!("{error}, so no unit is known yet"));
            return;
        }
    };
    let request = UpRequest {
        asked,
        home: std::env::var_os("HOME").map(PathBuf::from),
    };
    let planned = match planned(layout.root(), &request) {
        Ok(plan) => plan,
        Err(unplanned) => {
            log(&format!(
                "the definitions do not plan, so no unit is known yet:\n{}",
                unplanned.lines.join("\n")
            ));
            return;
        }
    };
    if planned.uses_host_listen {
        supervisor.learn_host_listen(Occasion::FindingAgain).await;
    }
    let plan: Vec<Arc<PlannedUnit>> = planned.units.into_iter().map(Arc::new).collect();
    supervisor.take_in(&plan);
    let up = match Intent::read(layout) {
        Ok(intent) => intent == Intent::Up,
        Err(error) => {
            log(&format!("{error}; the environment is taken to be down"));
            false
        }
    };
    engine::find_again(supervisor, &plan, up).await;
    if let Err(why) = engine::find_unplanned(supervisor).await {
        log(&format!(
            "what is left of units the plan does not have cannot be told: {why}"
        ));
    }
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

    /// Tells the followers of the change under way, if any, that `unit`
    /// took `state`.
    fn tell(&self, unit: &str, state: &UnitState) {
        if let Some(change) = self
            .change
            .lock()
            .expect("the change under way is not poisoned")
            .as_ref()
        {
            change.say(Event::State {
                unit: unit.to_string(),
                state: state.state,
                detail: state.detail.clone(),
                at: clock::now(),
            });
        }
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

/// A daemon's log longer than this is set aside and a new one begun, so
/// that an environment brought up for months does not fill the disk. One
/// file set aside is kept.
const LOG_SIZE: u64 = 1 << 20;

/// The daemon's log, the path it is at, once the daemon runs.
static LOG: OnceLock<PathBuf> = OnceLock::new();

/// The daemon's log at `path`, opened to be added to; a long one is set
/// aside first.
pub fn open_log(path: &Path) -> Result<File, DiskError> {
    let folder = path
        .parent()
        .expect("the daemon's log is in the logs folder");
    std::fs::create_dir_all(folder).map_err(DiskError::io(folder, "made"))?;
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > LOG_SIZE) {
        let set_aside = path.with_extension("log.1");
        std::fs::rename(path, &set_aside).map_err(DiskError::io(path, "set aside"))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(DiskError::io(path, "opened"))
}

/// Writes a line to the daemon's log, which is its standard error, with its
/// time, and begins a new log when it has grown too long.
fn log(line: &str) {
    print::err(&format!("{}  {line}\n", clock::utc(clock::now())));
    let Some(path) = LOG.get() else {
        return;
    };
    let long = rustix::fs::fstat(rustix::stdio::stderr())
        .is_ok_and(|stat| u64::try_from(stat.st_size).is_ok_and(|size| size > LOG_SIZE));
    // A log whose folder has gone, with the environment's, is not made
    // again.
    if long && path.parent().is_some_and(Path::exists) {
        // A log that cannot be begun anew is written on as it is: there is
        // nowhere else to say so.
        if let Ok(file) = open_log(path) {
            let _ = rustix::stdio::dup2_stderr(&file);
        }
    }
}

fn log_state(unit: &str, state: &UnitState) {
    log(&format!(
        "{unit}: {} ({})",
        state.state.word(),
        state.detail
    ));
}
