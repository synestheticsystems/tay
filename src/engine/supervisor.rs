//! The units of an environment as its daemon looks after them: the state of
//! each, and for each unit that runs, a task of its own on the daemon's
//! runtime that drives its attempts, checks its readiness for as long as it
//! runs, restarts it as its policy says, and stops it when asked. Every
//! change of state is recorded here, and told to the daemon as it happens.
//!
//! The tasks live as long as what they look after runs, which may be long
//! after any change that started them, and they end with the daemon, which
//! leaves what they look after running.

mod tending;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot, watch};

use super::fill::UnitValues;
use super::state::{State, UnitState};
use super::units::Units;
use crate::disk::Environment;
use crate::disk::unit_log::UnitLog;
use crate::module::{self, Running, Runs, UnitKind, UnitPlace, Values};
use crate::plan::PlannedUnit;

pub use tending::Begin;

/// How many of the last lines a unit wrote are shown when it fails.
const LAST_LINES: usize = 10;

/// What the daemon is told of each change of state.
pub type Told = Box<dyn Fn(&str, &UnitState) + Send + Sync>;

pub struct Supervisor {
    pub environment: Arc<Environment>,
    units: Mutex<Units>,
    /// Counts the changes of state, for whoever waits on one.
    changes: watch::Sender<u64>,
    /// The task looking after each unit that runs, by unit.
    tending: Mutex<HashMap<String, Tended>>,
    told: Told,
}

/// A unit's task, whether it has ended, and how it is asked to stop.
struct Tended {
    ended: Arc<AtomicBool>,
    stop: mpsc::UnboundedSender<oneshot::Sender<Result<(), String>>>,
}

impl Tended {
    fn is_live(&self) -> bool {
        !self.ended.load(Ordering::SeqCst)
    }
}

impl Supervisor {
    pub fn new(environment: Arc<Environment>, told: Told) -> Supervisor {
        Supervisor {
            environment,
            units: Mutex::default(),
            changes: watch::Sender::new(0),
            tending: Mutex::default(),
            told,
        }
    }

    fn units(&self) -> std::sync::MutexGuard<'_, Units> {
        self.units.lock().expect("the units are not poisoned")
    }

    /// Takes in the units of a plan, as [`Units::take_in`] does.
    pub fn take_in(&self, plan: &[Arc<PlannedUnit>]) {
        self.units().take_in(plan);
    }

    /// The units as they stand now.
    pub fn snapshot(&self) -> Units {
        self.units().clone()
    }

    pub fn state(&self, unit: &str) -> Option<UnitState> {
        self.units().get(unit).map(|known| known.state.clone())
    }

    /// The values each unit that is ready gives.
    pub fn given(&self) -> UnitValues {
        self.units().given()
    }

    /// Records that `unit` took `state`, and what it gives if it gave
    /// anything, and tells the daemon when its state changed.
    pub fn record(&self, unit: &str, state: UnitState, values: Option<Values>) {
        let changed = self.units().record(unit, state.clone(), values);
        if changed {
            (self.told)(unit, &state);
            self.changes.send_modify(|count| *count += 1);
        }
    }

    /// Wakes when any unit's state changes.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// Whether a task looks after `unit`: something of it runs, or is to.
    pub fn is_tended(&self, unit: &str) -> bool {
        self.tending
            .lock()
            .expect("the tasks are not poisoned")
            .get(unit)
            .is_some_and(Tended::is_live)
    }

    /// Looks after `planned` with `running`, from `begin`, unless a task
    /// does already. `restarts` is whether its restart policy applies, as
    /// it does when the environment is meant to be up.
    pub fn tend(
        self: &Arc<Self>,
        planned: &Arc<PlannedUnit>,
        running: Arc<dyn Running>,
        begin: Begin,
        restarts: bool,
    ) {
        let mut tending = self.tending.lock().expect("the tasks are not poisoned");
        if tending.get(&planned.name).is_some_and(Tended::is_live) {
            return;
        }
        // A unit started afresh is starting from now, before its task runs,
        // so that no one reads the state it had before.
        if let Begin::Fresh = begin {
            self.record(
                &planned.name,
                UnitState::new(State::Starting, "attempt 1, starting"),
                None,
            );
        }
        let (stop, stops) = mpsc::unbounded_channel();
        let tended = tending::tend(
            Arc::clone(self),
            Arc::clone(planned),
            running,
            begin,
            restarts,
            stops,
        );
        let supervisor = Arc::clone(self);
        let ended = Arc::new(AtomicBool::new(false));
        let ends = Arc::clone(&ended);
        tokio::spawn(async move {
            tended.await;
            // A unit no longer looked after is a change to whoever waits on
            // it to settle.
            ends.store(true, Ordering::SeqCst);
            supervisor.changes.send_modify(|count| *count += 1);
        });
        tending.insert(planned.name.clone(), Tended { ended, stop });
    }

    /// Stops `planned` and whatever it started, and waits until nothing of
    /// it is left; why not, when something is. A unit no task looks after
    /// is stopped from its place alone, whatever its settings.
    pub async fn stop(&self, planned: &PlannedUnit) -> Result<(), String> {
        let tended = self
            .tending
            .lock()
            .expect("the tasks are not poisoned")
            .get(&planned.name)
            .filter(|tended| tended.is_live())
            .map(|tended| tended.stop.clone());
        if let Some(stop) = tended {
            let (said, heard) = oneshot::channel();
            // A task that ended meanwhile has stopped what it looked after.
            if stop.send(said).is_ok()
                && let Ok(stopped) = heard.await
            {
                return stopped;
            }
        }
        let Some(runs) = runs(planned) else {
            return Ok(());
        };
        stop_blocking(runs.clone(), self.place(&planned.name)).await
    }

    /// The units of which something runs now, found where units run,
    /// whether or not the plan has them.
    pub fn found_running(&self) -> Result<Vec<(String, Runs)>, String> {
        module::found_running(&self.environment.layout).map_err(|failure| failure.to_string())
    }

    /// Stops what runs of `unit`, which the plan does not have, from its
    /// place alone, where `runs` says it runs.
    pub async fn stop_unplanned(&self, unit: &str, runs: Runs) -> Result<(), String> {
        stop_blocking(runs, self.place(unit)).await
    }

    /// Whether anything of `planned` runs now, from its place alone,
    /// whatever its settings.
    pub async fn running_now(&self, planned: &PlannedUnit) -> Result<bool, String> {
        let Some(runs) = runs(planned).cloned() else {
            return Ok(false);
        };
        let place = self.place(&planned.name);
        tokio::task::spawn_blocking(move || module::running_now(&runs, &place))
            .await
            .map_err(|error| format!("tay failed while looking at it: {error}"))?
            .map_err(|failure| failure.to_string())
    }

    /// Where `unit` is acted on.
    pub fn place(&self, unit: &str) -> UnitPlace {
        UnitPlace {
            unit: unit.to_string(),
            environment: Arc::clone(&self.environment),
        }
    }

    /// The last lines `unit` wrote, as its log holds them.
    pub fn last_lines(&self, unit: &str) -> Vec<String> {
        let log = UnitLog::at(self.environment.layout.unit_log(unit));
        match log.last(LAST_LINES) {
            Ok(lines) => lines.iter().map(|line| line.whole()).collect(),
            Err(error) => vec![format!("its log cannot be read: {error}")],
        }
    }
}

/// Where a unit that runs runs; `None` for one that is made, or a group.
fn runs(planned: &PlannedUnit) -> Option<&Runs> {
    match &planned.mechanism.as_ref()?.kind {
        UnitKind::Runs(runs) => Some(runs),
        UnitKind::Made => None,
    }
}

/// Stops what of a unit runs, on the blocking pool, since stopping waits
/// for its processes to end.
async fn stop_blocking(runs: Runs, place: UnitPlace) -> Result<(), String> {
    tokio::task::spawn_blocking(move || module::stop(&runs, &place))
        .await
        .map_err(|error| format!("tay failed while stopping it: {error}"))?
        .map_err(|failure| failure.to_string())
}
