//! The daemon as `tay serve` runs it: in the foreground, for a service
//! manager to start, restart and stop. Being started is being asked to be
//! up, so it brings up what the environment remembers being asked for,
//! whatever was recorded of whether it is meant to be up. Its journal is
//! told each change of state for as long as it runs. Told to stop, it takes
//! the environment down as `tay down` does.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::signal::unix::{Signal, SignalKind, signal};

use super::change::{Change, Kind};
use super::down::Down;
use super::{CannotStart, Daemon, Exiting, Journal, listen, log, start};
use crate::control::client::Place;
use crate::control::protocol::UpRequest;
use crate::disk::{Asked, Layout};
use crate::engine::Left;

/// How `tay serve` ended, once it was the daemon.
pub enum Served {
    /// It took the environment down, told to stop or by `tay down` or
    /// `tay wipe --all`: what was left that could not be stopped, or why
    /// that could not be told.
    Down(Result<Vec<Left>, String>),
    /// It could not go on, and why, as a person reads it.
    CannotGoOn(String),
}

/// Runs as the daemon of the environment at `layout`, in the `place` taken
/// for it, until the daemon exits, heeding `signals`. What it brings up is
/// chosen with the choices of whoever has `home`; `journal` is told each
/// change of state, and what the bring-up it begins with says.
pub async fn serve(
    layout: Layout,
    place: Place,
    signals: Signals,
    home: Option<PathBuf>,
    journal: Journal,
) -> Result<Served, CannotStart> {
    super::log::in_own_file(layout.daemon_log())?;
    let (daemon, listener, socket) = match start(layout, Some(Arc::clone(&journal))).await {
        Ok(started) => started,
        Err(why) => {
            log(&format!("cannot start: {why}"));
            return Err(why);
        }
    };
    Arc::clone(&daemon).bring_up_served(home, journal);
    // It answers from now on.
    drop(place.bootstrap);
    // Told to stop while it started, it stops now.
    let stopping = tokio::spawn(stop_when_told(Arc::clone(&daemon), signals));
    let exiting = listen::listen(&daemon, listener, &place.daemon, &socket).await;
    stopping.abort();
    Ok(match exiting {
        Some(Exiting::TookDown {
            because: Down::Gone,
            ..
        }) => Served::CannotGoOn(
            "the environment's folder has gone, so what it ran was stopped".to_string(),
        ),
        Some(Exiting::TookDown { left, .. }) => Served::Down(left),
        Some(Exiting::MakingWay) => Served::CannotGoOn(
            "it was asked to exit, so that another daemon takes its place: one of another version of tay, or another `tay serve`; the stack runs on".to_string(),
        ),
        Some(Exiting::SocketLost) => Served::CannotGoOn(
            "its socket was removed or replaced, so no command could reach it; the stack runs on"
                .to_string(),
        ),
        None => Served::CannotGoOn("tay ended the daemon without saying why".to_string()),
    })
}

/// The signals `tay serve` heeds, from its first moment: `SIGTERM`, which
/// a service manager sends, and `SIGINT`, which a terminal sends when a
/// person presses control-C, tell it to stop. `SIGHUP`, which a terminal
/// sends when it closes, is no one asking for the stack to stop, and it
/// carries on; ended by it, it would leave the stack running with no one to
/// look after it, and systemd, which takes that end for a clean one, would
/// not start it again.
pub struct Signals {
    terminate: Signal,
    interrupt: Signal,
    hangup: Signal,
}

impl Signals {
    /// Listens for the signals from now on, so that one that comes before
    /// the daemon has started is kept until it has.
    pub fn new() -> Result<Signals, CannotStart> {
        let listen = |kind| signal(kind).map_err(CannotStart::Signals);
        Ok(Signals {
            terminate: listen(SignalKind::terminate())?,
            interrupt: listen(SignalKind::interrupt())?,
            hangup: listen(SignalKind::hangup())?,
        })
    }
}

/// Takes the environment down once a signal to stop comes, unless the
/// daemon is exiting already. Later signals are not heeded: the stack is
/// being stopped.
async fn stop_when_told(daemon: Arc<Daemon>, mut signals: Signals) {
    let signal = loop {
        tokio::select! {
            _ = signals.terminate.recv() => break "SIGTERM",
            _ = signals.interrupt.recv() => break "SIGINT",
            _ = signals.hangup.recv() => {
                log("SIGHUP is not a signal to stop: the daemon carries on");
            }
        }
    };
    log(&format!("told to stop by {signal}"));
    if daemon.exit.borrow().is_none() {
        daemon.begin_down(Down::Stopped);
    }
}

impl Daemon {
    /// Begins bringing up what the environment remembers being asked for,
    /// as a plain `tay up` run from `home` would, so that such a `tay up`
    /// joins it. `journal` is told what it says.
    fn bring_up_served(self: Arc<Self>, home: Option<PathBuf>, journal: Journal) {
        let request = UpRequest {
            adds: Asked::default(),
            home,
        };
        let change = Change::journalled(Kind::Serve(request.clone()), journal);
        // The daemon answers no one before it listens, so no other change
        // is under way.
        *self
            .change
            .lock()
            .expect("the change under way is not poisoned") = Some(Arc::clone(&change));
        tokio::spawn(self.bring_up(change, request));
    }
}
