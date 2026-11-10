//! Restarting units: `tay restart` as the daemon makes it. Each unit named
//! is stopped and then brought up again, as `tay up` brings it up, with a
//! new first attempt, from the plan the daemon holds; what it needs is
//! brought up with it, and units that need it are left as they run.

use std::sync::Arc;

use super::change::Change;
use super::{Daemon, log};
use crate::control::protocol::{Event, Restart};
use crate::disk::Intent;
use crate::engine::{State, UnitState, runs};
use crate::plan::PlannedUnit;

/// What a restart acts on: the units named, and with them what they need,
/// in the order units start in.
pub struct Restarting {
    named: Vec<Arc<PlannedUnit>>,
    with_needs: Vec<Arc<PlannedUnit>>,
}

impl Daemon {
    /// What `restart` acts on; why it cannot be made, when it cannot: the
    /// environment is not meant to be up, or a unit named is not one that
    /// runs in the plan the daemon holds.
    pub fn restarting(&self, restart: &Restart) -> Result<Restarting, String> {
        let intent = Intent::read(&self.environment.layout).map_err(|error| error.to_string())?;
        if intent != Intent::Up {
            return Err("the environment is down: `tay up` brings it up".to_string());
        }
        if restart.units.is_empty() {
            return Err("name each unit to restart".to_string());
        }
        let units = self.supervisor.snapshot();
        let mut named = Vec::new();
        for unit in &restart.units {
            let known = units
                .get(unit)
                .ok_or_else(|| format!("there is no unit `{unit}` in this environment's plan"))?;
            if runs(&known.planned).is_none() {
                return Err(format!(
                    "`{unit}` runs nothing, so it has nothing to restart: only a process, a container or a command does"
                ));
            }
            named.push(Arc::clone(&known.planned));
        }
        Ok(Restarting {
            named,
            with_needs: units.with_needs(&restart.units),
        })
    }

    /// Stops each unit named and brings it up again with what it needs,
    /// telling `change`'s followers each change of state and what it came
    /// to.
    pub async fn restart(self: Arc<Self>, change: Arc<Change>, restarting: Restarting) {
        log(&format!("change: {}", change.describe()));
        change.say(Event::Units {
            units: (restarting.with_needs.iter())
                .map(|planned| planned.name.clone())
                .collect(),
        });
        for planned in &restarting.named {
            if let Err(why) = self.supervisor.stop_to_replace(planned).await {
                let reason = format!("`{}` could not be stopped: {why}", planned.name);
                self.fail(&change, &reason);
                return;
            }
            let stopped = UnitState::new(State::Stopped, "stopped by `tay restart`");
            self.supervisor.record(&planned.name, stopped, None);
        }
        let named: Vec<String> = (restarting.named.iter())
            .map(|planned| planned.name.clone())
            .collect();
        self.settle(&change, &restarting.with_needs, &named).await;
    }
}
