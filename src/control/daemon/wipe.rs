//! Wiping: `tay wipe --yes` as the daemon makes it. Of units named, each
//! running unit that needs one is stopped, then each named unit, then what
//! each keeps is removed, as its module answers, and on an environment
//! meant to be up everything is brought up again as `tay up` would. A unit
//! whose removal runs something of its own, as a command's `undo`, needs
//! what it needs to be up, and the wipe is refused when it is not. With
//! `--all`, the environment is taken down and everything it keeps removed:
//! every volume, and `.tay/` but for `choices.yaml`; then the daemon exits.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::change::{Change, NotBegun};
use super::down::Down;
use super::{Daemon, log, options};
use crate::control::protocol::{Event, FOLDER_WIPED, LISTING_WIPED, Refused};
use crate::disk::{Asked, Intent, List};
use crate::engine::{State, UnitState, runs};
use crate::module;
use crate::plan::PlannedUnit;

/// What a wipe of units acts on: the units named, and each that needs one
/// of them, in the order units start in.
pub struct Wiping {
    named: Vec<Arc<PlannedUnit>>,
    needing: Vec<Arc<PlannedUnit>>,
    /// The home folder whose choices the stack is planned with again.
    home: Option<PathBuf>,
}

impl Daemon {
    /// What a wipe of `units` acts on; why it is not begun, when it is not:
    /// a unit named is not in the plan the daemon holds, or is a group, or
    /// its removal runs something that needs a unit that is not up.
    pub fn wiping(&self, units: &[String], home: Option<PathBuf>) -> Result<Wiping, NotBegun> {
        let does_not_fit = NotBegun::DoesNotFit;
        if units.is_empty() {
            return Err(does_not_fit("name each unit to wipe".to_string()));
        }
        let table = self.supervisor.snapshot();
        let mut named = Vec::new();
        for unit in units {
            let known = table.get(unit).ok_or_else(|| {
                does_not_fit(format!(
                    "there is no unit `{unit}` in this environment's plan"
                ))
            })?;
            if known.planned.mechanism.is_none() {
                return Err(does_not_fit(format!(
                    "`{unit}` is a group, which keeps nothing of its own"
                )));
            }
            named.push(Arc::clone(&known.planned));
        }
        let needing = table.needing(units);
        let stopped = |unit: &str| {
            units.iter().any(|named| named == unit)
                || needing.iter().any(|needing| needing.name == unit)
        };
        for planned in &named {
            if !self.supervisor.kept(planned).runs {
                continue;
            }
            for need in &planned.needs {
                let ready = table
                    .state(&need.unit)
                    .is_some_and(|state| state.state == State::Ready);
                if !ready || stopped(&need.unit) {
                    return Err(NotBegun::Refused(Refused::NotUp {
                        problem: format!(
                            "wiping `{}` runs what undoes its work, which needs `{}`, and `{}` is not up",
                            planned.name, need.unit, need.unit
                        ),
                    }));
                }
            }
        }
        Ok(Wiping {
            named,
            needing,
            home,
        })
    }

    /// Stops each unit that needs one named and each named, removes what
    /// each named keeps, and brings everything up again if the environment
    /// is meant to be up, telling `change`'s followers each change of state
    /// and what was removed.
    pub async fn wipe(self: Arc<Self>, change: Arc<Change>, wiping: Wiping) {
        log(&format!("change: {}", change.describe()));
        change.say(Event::Units {
            units: (wiping.needing.iter())
                .chain(&wiping.named)
                .map(|planned| planned.name.clone())
                .collect(),
        });
        // What holds what is removed is stopped first, the last to start
        // the first to stop.
        let needing = wiping.needing.iter().rev();
        let running = needing.filter(|planned| self.supervisor.is_tended(&planned.name));
        for planned in running.chain(wiping.named.iter().rev()) {
            if runs(planned).is_none() {
                continue;
            }
            if let Err(why) = self.supervisor.stop_to_replace(planned).await {
                let reason = format!("`{}` could not be stopped: {why}", planned.name);
                self.fail(&change, &reason);
                return;
            }
            let stopped = UnitState::new(State::Stopped, "stopped by `tay wipe`");
            self.supervisor.record(&planned.name, stopped, None);
        }
        for planned in &wiping.named {
            let kept = self.supervisor.kept(planned);
            if let Err(why) = self.supervisor.wipe(planned).await {
                let reason = format!(
                    "what `{}` keeps could not be removed: {why}; what was stopped for the wipe stays stopped until `tay up`",
                    planned.name
                );
                self.fail(&change, &reason);
                return;
            }
            let wiped = UnitState::new(State::Stopped, "what it kept was removed by `tay wipe`");
            self.supervisor.record(&planned.name, wiped, None);
            change.say(Event::Removed {
                unit: Some(planned.name.clone()),
                removed: kept.what,
            });
        }
        let layout = &self.environment.layout;
        let up = match Intent::read(layout) {
            Ok(intent) => intent == Intent::Up,
            Err(error) => {
                self.fail(&change, &error.to_string());
                return;
            }
        };
        if !up {
            change.say(Event::Wiped);
            self.finish(&change);
            return;
        }
        let remembered = match Asked::read(layout) {
            Ok(remembered) => remembered,
            Err(error) => {
                self.fail(&change, &error.to_string());
                return;
            }
        };
        if let Some(plan) = self.plan(&change, options(remembered, wiping.home)).await {
            self.bring_up_plan(&change, plan).await;
        }
    }

    /// Takes the environment down and removes everything it keeps, telling
    /// `change`'s followers each change of state and what was removed; then
    /// the daemon exits, as after `tay down`. While anything could not be
    /// stopped, nothing is removed.
    pub async fn wipe_all(self: Arc<Self>, change: Arc<Change>) {
        log(&format!("change: {}", change.describe()));
        let environment = &self.environment;
        if let Err(error) = Intent::Down.write(&environment.layout) {
            self.fail(&change, &error.to_string());
            self.exit(Down::Wiped.exit());
            return;
        }
        let stopped = self.stop_everything(&change, Down::Wiped).await;
        let removed = match stopped {
            Ok(left) if left.is_empty() => self.remove_everything().await,
            Ok(left) => {
                change.say(Event::Down { left });
                Err("something was left, so nothing was removed".to_string())
            }
            Err(problem) => Err(problem),
        };
        match removed {
            Ok(removed) => {
                change.say(Event::Removed {
                    unit: None,
                    removed,
                });
                change.say(Event::Wiped);
                self.finish(&change);
            }
            Err(problem) => self.fail(&change, &problem),
        }
        self.exit(Down::Wiped.exit());
    }

    /// Removes everything the environment keeps, once nothing of it runs:
    /// its volumes, `.tay/` but for `choices.yaml`, and its place on the
    /// machine's list with the ports it kept. Gives what was removed.
    async fn remove_everything(&self) -> Result<Vec<String>, String> {
        let environment = &self.environment;
        let mut removed = module::remove_kept_by_environment(environment)
            .await
            .map_err(|failure| failure.to_string())?;
        // Its folder goes now, which the daemon is not to take for the
        // environment gone.
        self.going.store(true, Ordering::SeqCst);
        environment
            .layout
            .remove_kept()
            .map_err(|error| error.to_string())?;
        // The list is changed under its lock, which may be waited for.
        let (machine, id) = (environment.machine.clone(), environment.id.clone());
        tokio::task::spawn_blocking(move || List::take_off(&machine, &id))
            .await
            .map_err(|error| format!("tay failed while changing the machine's list: {error}"))?
            .map_err(|error| error.to_string())?;
        removed.extend([FOLDER_WIPED, LISTING_WIPED].map(str::to_string));
        Ok(removed)
    }
}
