//! Taking the environment down: `tay down`, `tay serve` told to stop, or
//! what the daemon does when the environment's folder has gone, or once a
//! change on an environment that is down leaves nothing of it running. Each
//! stops every unit and the daemon exits.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::change::{Change, Kind};
use super::{Daemon, Exiting, log};
use crate::control::protocol::Event;
use crate::disk::Intent;
use crate::engine;

/// Why the daemon takes the environment down.
#[derive(Clone, Copy)]
pub enum Down {
    /// `tay down` asked.
    Asked,
    /// `tay serve` was told to stop, by the signal a service manager sends
    /// or by control-C in a terminal.
    Stopped,
    /// The environment's `.tay/` folder has gone, or the stack's root with
    /// it.
    Gone,
    /// `tay wipe --all` asked, which then removes what the environment
    /// keeps.
    Wiped,
    /// A change on an environment that is down is over, and nothing of it
    /// runs.
    Unneeded,
}

impl Down {
    /// Why each unit is stopped, as its state's detail says it.
    fn detail(self) -> &'static str {
        match self {
            Down::Asked => "stopped by `tay down`",
            Down::Stopped => "stopped: `tay serve` was told to stop",
            Down::Gone => "stopped: the environment's folder has gone",
            Down::Wiped => "stopped by `tay wipe --all`",
            Down::Unneeded => "stopped: the environment is down",
        }
    }

    /// Why the daemon exits, as its log says it.
    pub(super) fn exit(self) -> &'static str {
        match self {
            Down::Asked => "taken down by `tay down`",
            Down::Stopped => "`tay serve` was told to stop",
            Down::Gone => "the environment's folder has gone",
            Down::Wiped => "taken down by `tay wipe --all`",
            Down::Unneeded => "the environment is down and nothing of it runs",
        }
    }

    /// What cancels the change under way, as a person reads it.
    fn cancels(self) -> &'static str {
        match self {
            Down::Asked => "`tay down`",
            Down::Stopped => "`tay serve` being told to stop",
            Down::Gone => "the environment's folder going",
            Down::Wiped => "`tay wipe --all`",
            Down::Unneeded => "the environment being down",
        }
    }
}

impl Daemon {
    /// Begins taking the environment down, `because` says why, cancelling
    /// the change under way; or joins the down under way. Gives the down.
    pub(super) fn begin_down(self: &Arc<Self>, because: Down) -> Arc<Change> {
        let mut under_way = self
            .change
            .lock()
            .expect("the change under way is not poisoned");
        let change = match under_way.take() {
            Some(change) if change.kind == Kind::Down => change,
            before => {
                let change = Change::new(Kind::Down);
                tokio::spawn(Arc::clone(self).take_down(Arc::clone(&change), before, because));
                change
            }
        };
        *under_way = Some(Arc::clone(&change));
        change
    }

    /// Stops every unit, once the change that was under way, if any, has
    /// stopped, and then the daemon exits.
    pub async fn take_down(
        self: Arc<Self>,
        change: Arc<Change>,
        before: Option<Arc<Change>>,
        because: Down,
    ) {
        log(&format!("change: {}", change.describe()));
        if let Some(before) = before {
            before.cancel(because.cancels());
            before.ended().await;
        }
        // What a person or a service manager takes down is meant to be
        // down, so that no later command brings it up unasked. A folder
        // that has gone is not made again to say so.
        if let Down::Asked | Down::Stopped = because
            && let Err(error) = Intent::Down.write(&self.environment.layout)
        {
            log(&format!(
                "the environment's intent cannot be written: {error}"
            ));
        }
        let left = self.stop_everything(&change, because).await;
        change.say(match &left {
            Ok(left) => Event::Down { left: left.clone() },
            Err(problem) => Event::Problem {
                problem: problem.clone(),
            },
        });
        self.finish(&change);
        self.exit(Exiting::TookDown { because, left });
    }

    /// The environment's `.tay/` folder has gone while the stack runs: the
    /// daemon takes it down, stopping everything it started, and exits.
    pub fn environment_gone(self: &Arc<Self>) {
        if self.going.swap(true, Ordering::SeqCst) {
            return;
        }
        log("the environment's folder has gone");
        let change = Change::new(Kind::Down);
        let before = self
            .change
            .lock()
            .expect("the change under way is not poisoned")
            .replace(Arc::clone(&change));
        tokio::spawn(Arc::clone(self).take_down(change, before, Down::Gone));
    }

    /// Stops every unit, telling `change`'s followers each change; what
    /// could not be stopped, or was left.
    pub(super) async fn stop_everything(
        &self,
        change: &Arc<Change>,
        because: Down,
    ) -> Result<Vec<engine::Left>, String> {
        let units = self.supervisor.snapshot();
        change.say(Event::Units {
            units: units
                .iter()
                .map(|known| known.planned.name.clone())
                .collect(),
        });
        engine::take_down(&self.supervisor, because.detail()).await
    }
}
