//! An environment's daemon: one per environment, started detached by the
//! first command that needs it as `tay daemon`, or run in the foreground by
//! `tay serve` for a service manager. It owns the changes to the
//! environment, listens on a Unix socket in a short folder named by the
//! environment's id, and lives for as long as it holds `.tay/daemon.lock`.
//!
//! One started detached tells the command that started it whether it is
//! ready by one line on its standard output, which that command reads
//! through a pipe: `ready` once it listens, `held` when another daemon holds
//! the lock, or `error:` and why it cannot run. Everything else it says goes
//! to its log, which is its standard error.

mod change;
mod down;
mod listen;
mod log;
mod restart;
mod routes;
mod run;
mod served;
mod up;
mod wipe;

use std::fmt;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Weak};

use tokio::net::UnixListener;
use tokio::sync::watch;

use crate::clock;
use crate::control::protocol::{self, Event, Refused};
use crate::definition::read::THIS_TAY;
use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Sources;
use crate::disk::{
    self, Asked, Counts, DiskError, Environment, Identity, Intent, Layout, Lock, Machine,
};
use crate::engine::{self, Left, Supervisor, Told, UnitState};
use crate::module::Occasion;
use crate::plan::{self, Options, Plan, PlannedUnit};

use change::{Change, NotBegun};
use down::Down;
use log::log;

pub use log::open_log;
pub use served::{Served, Signals, serve};

/// What `tay serve` prints for as long as it runs: each change of state,
/// and what the changes it makes itself say.
pub type Journal = Arc<dyn Fn(Event) + Send + Sync>;

/// What the daemon holds while it runs.
pub struct Daemon {
    environment: Arc<Environment>,
    about: protocol::About,
    /// The units, and the tasks that look after those that run.
    supervisor: Arc<Supervisor>,
    /// The change under way, if any.
    change: Mutex<Option<Arc<Change>>>,
    /// Why the daemon is to exit, once it is.
    exit: watch::Sender<Option<Exiting>>,
    /// Whether it has begun to stop everything because the environment's
    /// folder has gone.
    going: AtomicBool,
    /// What `tay serve` prints, for a daemon it runs.
    journal: Option<Journal>,
}

/// Why the daemon exits.
#[derive(Clone)]
enum Exiting {
    /// It took the environment down, for the reason `because` gives: what
    /// was left that could not be stopped, or why that could not be told.
    TookDown {
        because: Down,
        left: Result<Vec<Left>, String>,
    },
    /// It was asked to, so that another daemon takes its place: one of
    /// another version of tay, or one `tay serve` runs. The stack runs on.
    MakingWay,
    /// Its socket was removed or replaced, so no command can reach it. The
    /// stack runs on.
    SocketLost,
}

impl fmt::Display for Exiting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Exiting::TookDown { because, .. } => f.write_str(because.exit()),
            Exiting::MakingWay => f.write_str("asked to, so that another daemon takes its place"),
            Exiting::SocketLost => {
                f.write_str("its socket was removed or replaced, so no command can reach it")
            }
        }
    }
}

/// Why the daemon cannot start.
#[derive(Debug, thiserror::Error)]
pub enum CannotStart {
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
    #[error("tay cannot be told to stop: {0}")]
    Signals(std::io::Error),
}

/// Runs the daemon of the environment whose stack's root is `root`,
/// started detached, until it is taken down, asked to exit, or its folder
/// or socket disappears. Gives whether it could run; one that finds
/// another daemon running leaves it to that one, which is as it should be.
pub fn run(root: &Path) -> bool {
    let layout = Layout::of(root);
    // The command that starts the daemon gives it this log as its standard
    // error.
    log::on_stderr(layout.daemon_log());
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
        let (daemon, listener, socket) = match start(layout, None).await {
            Ok(started) => started,
            Err(why) => return cannot_start(why),
        };
        ready_line("ready");
        listen::listen(&daemon, listener, &lock, &socket).await;
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

/// Becomes the daemon of the environment at `layout`, whose lock is held:
/// listens on its socket, and finds its units again. `journal` is what
/// `tay serve` prints, for a daemon it runs, and is told each change of
/// state.
async fn start(
    layout: Layout,
    journal: Option<Journal>,
) -> Result<(Arc<Daemon>, UnixListener, Socket), CannotStart> {
    let machine = Machine::from_env()?;
    let Identity::Own(environment) = Environment::identify(&layout, &machine)? else {
        return Err(CannotStart::NotIdentified);
    };
    machine.make_socket_folder(environment.id.as_str())?;
    let about = protocol::About {
        tay: THIS_TAY.to_string(),
        pid: std::process::id(),
        root: environment.layout.root().to_path_buf(),
        id: environment.id.to_string(),
        started: clock::now(),
        // Only a daemon `tay serve` runs has a journal.
        served: journal.is_some(),
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
            journal,
        }
    });
    found_again(&daemon.supervisor).await;
    // The socket is made once the daemon can answer on it: until then a
    // command finds nothing listening, and waits for the daemon's lock
    // rather than for an answer that would not come in time.
    let (listener, socket) = listen_on(daemon.environment.socket())?;
    Ok((daemon, listener, socket))
}

/// Listens on the socket at `path`, which this account alone may use.
fn listen_on(path: PathBuf) -> Result<(UnixListener, Socket), CannotStart> {
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
    let identity = (metadata.dev(), metadata.ino());
    Ok((listener, Socket { path, identity }))
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
    let options = options(asked, std::env::var_os("HOME").map(PathBuf::from));
    let planned = match planned(layout.root(), &options) {
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
    let production = planned.production;
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
    // Nothing on a server is watched.
    if up && !production {
        for problem in supervisor.watch(&plan).await {
            log(&problem);
        }
    }
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

/// What the daemon plans with: `asked`, with the choices of whoever has
/// `home`, and this machine's folder of shared folders.
fn options(asked: Asked, home: Option<PathBuf>) -> Options {
    Options {
        asked,
        home,
        shared_folders: disk::shared_folders(),
        action: None,
    }
}

/// Plans the stack at `root` as `tay plan` would with `options`.
fn planned(root: &Path, options: &Options) -> Result<Plan, Unplanned> {
    let mut sources = Sources::default();
    let mut d = Diagnostics::default();
    let rendered = |errors: Vec<crate::diagnostic::Diagnostic>, sources: &Sources| {
        errors.iter().map(|error| error.render(sources)).collect()
    };
    match plan::plan(root, options, &mut sources, &mut d) {
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
    /// Refuses a change that brings up units unless the environment is
    /// meant to be up, so that no unit is started against its intent.
    fn meant_to_be_up(&self) -> Result<(), NotBegun> {
        match Intent::read(&self.environment.layout) {
            Ok(Intent::Up) => Ok(()),
            Ok(Intent::Down) => Err(NotBegun::Refused(Refused::environment_down())),
            Err(error) => Err(NotBegun::Refused(Refused::NotUp {
                problem: format!(
                    "whether the environment is meant to be up cannot be told: {error}"
                ),
            })),
        }
    }

    /// Asks the daemon to exit, once the answers under way have finished,
    /// for the reason `why` gives. What runs is left as it is, and a change
    /// under way stops.
    fn exit(&self, why: Exiting) {
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
                *exit = Some(why);
            }
            first
        });
    }

    /// Tells the journal, for a daemon `tay serve` runs, and the followers
    /// of the change under way, if any, that `unit` took `state`.
    fn tell(&self, unit: &str, state: &UnitState) {
        let event = Event::State {
            unit: unit.to_string(),
            state: state.state,
            detail: state.detail.clone(),
            at: clock::now(),
        };
        if let Some(journal) = &self.journal {
            journal(event.clone());
        }
        if let Some(change) = self
            .change
            .lock()
            .expect("the change under way is not poisoned")
            .as_ref()
        {
            change.say(event);
        }
    }

    /// Once a change is over, takes the environment down as `tay down`
    /// does, and so exits, if it is down and nothing of it runs: a daemon
    /// started for a change on an environment that is down is not left
    /// behind, nor what its units share where they run.
    async fn leave_if_unneeded(self: Arc<Self>) {
        let down = matches!(Intent::read(&self.environment.layout), Ok(Intent::Down));
        if !down || self.supervisor.tends_any() {
            return;
        }
        // What runs that no task looks after, as what a daemon found of
        // units its plan lacks, keeps the daemon too.
        match self.supervisor.found_left().await {
            Ok(left) if left.is_empty() => {}
            Ok(_) => return,
            Err(why) => {
                log(&format!(
                    "whether anything of the environment runs cannot be told, so the daemon stays: {why}"
                ));
                return;
            }
        }
        // It holds the place of a change, so that none begins meanwhile.
        let change = {
            let mut under_way = self
                .change
                .lock()
                .expect("the change under way is not poisoned");
            if under_way.is_some() || self.exit.borrow().is_some() {
                return;
            }
            let change = Change::new(change::Kind::Down);
            *under_way = Some(Arc::clone(&change));
            change
        };
        self.take_down(change, None, Down::Unneeded).await;
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

fn log_state(unit: &str, state: &UnitState) {
    log(&format!(
        "{unit}: {} ({})",
        state.state.word(),
        state.detail
    ));
}
