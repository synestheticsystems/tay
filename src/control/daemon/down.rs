//! Taking the environment down: `tay down`, or what the daemon does when the
//! environment's folder has gone. Either stops every unit and the daemon
//! exits.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::change::{Change, Kind};
use super::{Daemon, log};
use crate::control::protocol::Event;
use crate::disk::Intent;
use crate::engine;

/// Why the daemon takes the environment down.
#[derive(Clone, Copy)]
pub enum Down {
    /// `tay down` asked.
    Asked,
    /// The environment's `.tay/` folder has gone, or the stack's root with
    /// it.
    Gone,
    /// `tay wipe --all` asked, which then removes what the environment
    /// keeps.
    Wiped,
}

impl Down {
    /// Why each unit is stopped, as its state's detail says it.
    fn detail(self) -> &'static str {
        match self {
            Down::Asked => "stopped by `tay down`",
            Down::Gone => "stopped: the environment's folder has gone",
            Down::Wiped => "stopped by `tay wipe --all`",
        }
    }

    /// Why the daemon exits, as its log says it.
    pub(super) fn exit(self) -> &'static str {
        match self {
            Down::Asked => "taken down by `tay down`",
            Down::Gone => "the environment's folder has gone",
            Down::Wiped => "taken down by `tay wipe --all`",
        }
    }
}

impl Daemon {
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
            before.cancel("`tay down`");
            before.ended().await;
        }
        // A folder that has gone is not made again to say so.
        if let Down::Asked = because
            && let Err(error) = Intent::Down.write(&self.environment.layout)
        {
            log(&format!(
                "the environment's intent cannot be written: {error}"
            ));
        }
        match self.stop_everything(&change, because).await {
            Ok(left) => change.say(Event::Down { left }),
            Err(problem) => change.say(Event::Problem { problem }),
        }
        self.finish(&change);
        self.exit(because.exit());
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
