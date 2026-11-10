//! The units of an environment as its daemon looks after them: the state of
//! each, and for each unit that runs, a task of its own on the daemon's
//! runtime that drives its attempts, checks its readiness for as long as it
//! runs, restarts it as its policy says, and stops it when asked. Every
//! change of state is recorded here, and told to the daemon as it happens.
//!
//! The tasks live as long as what they look after runs, which may be long
//! after any change that started them, and they end with the daemon, which
//! leaves what they look after running.

mod restarts;
mod tending;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot, watch};

use super::fill::{self, HostListen, UnitValues};
use super::state::{State, UnitState};
use super::units::Units;
use crate::disk::unit_log::UnitLog;
use crate::disk::{Counts, Environment};
use crate::module::{self, Occasion, Running, Runs, UnitKind, UnitPlace, Values};
use crate::plan::PlannedUnit;
use crate::value::{Filled, Placeholder, Unfilled};

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
    tending: Mutex<HashMap<String, UnitTask>>,
    /// What is counted of each unit's attempts.
    counts: Counts,
    /// `${host.listen}`, once it has been learnt.
    host_listen: Mutex<HostListen>,
    told: Told,
}

/// A unit's task, whether it has ended, and how it is asked to stop.
struct UnitTask {
    ended: Arc<AtomicBool>,
    stop: mpsc::UnboundedSender<oneshot::Sender<Result<(), String>>>,
}

impl UnitTask {
    fn is_live(&self) -> bool {
        !self.ended.load(Ordering::SeqCst)
    }
}

impl Supervisor {
    pub fn new(environment: Arc<Environment>, counts: Counts, told: Told) -> Supervisor {
        Supervisor {
            environment,
            units: Mutex::default(),
            changes: watch::Sender::new(0),
            tending: Mutex::default(),
            counts,
            host_listen: Mutex::new(Err(
                "it is learnt when the environment is brought up".to_string()
            )),
            told,
        }
    }

    /// Learns `${host.listen}` where units run, on `occasion`.
    pub async fn learn_host_listen(&self, occasion: Occasion) {
        let learnt = module::host_listen(&self.environment, occasion)
            .await
            .map_err(|failure| failure.to_string());
        *self
            .host_listen
            .lock()
            .expect("the host's address is not poisoned") = learnt;
    }

    /// `${host.listen}`, as far as it has been learnt.
    pub fn host_listen(&self) -> HostListen {
        self.host_listen
            .lock()
            .expect("the host's address is not poisoned")
            .clone()
    }

    /// Fills `placeholder` for a unit whose needs have given `given`.
    pub fn fill(&self, placeholder: &Placeholder, given: &UnitValues) -> Result<Filled, Unfilled> {
        fill::fill(placeholder, &self.environment, &self.host_listen(), given)
    }

    fn units(&self) -> std::sync::MutexGuard<'_, Units> {
        self.units.lock().expect("the units are not poisoned")
    }

    /// Takes in the units of a plan, as [`Units::take_in`] does, giving
    /// each that comes back into it from among those set apart.
    pub fn take_in(&self, plan: &[Arc<PlannedUnit>]) -> Vec<Arc<PlannedUnit>> {
        self.units().take_in(plan)
    }

    /// The units as they stand now.
    pub fn snapshot(&self) -> Units {
        self.units().clone()
    }

    /// The state of `unit`, planned or not.
    pub fn state(&self, unit: &str) -> Option<UnitState> {
        self.units().state(unit).cloned()
    }

    /// Sets apart `unit`, which runs where `runs` says and which the plan
    /// does not have, in `state`, and tells the daemon of it, unless the
    /// table has it already.
    pub fn take_in_unplanned(&self, unit: &str, runs: Runs, state: UnitState) {
        if self.units().take_in_unplanned(unit, runs, state.clone()) {
            (self.told)(unit, &state);
            self.changes.send_modify(|count| *count += 1);
        }
    }

    /// Forgets the unplanned unit `unit`, of which nothing runs any more.
    pub fn forget(&self, unit: &str) {
        self.units().forget(unit);
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

    /// Records how the attempt of `unit` that came out last ended, before
    /// the state it takes is recorded, so that whoever waits on that state
    /// finds it.
    pub fn exited(&self, unit: &str, exit_status: Option<i64>) {
        self.units().exited(unit, exit_status);
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
            .is_some_and(UnitTask::is_live)
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
        if tending.get(&planned.name).is_some_and(UnitTask::is_live) {
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
        let unit = planned.name.clone();
        let task = tokio::spawn(tended);
        tokio::spawn(async move {
            // A task that panicked is a fault in tay, which fails its unit
            // rather than leave it looked after by nothing.
            if let Err(error) = task.await {
                let reason = format!("tay failed while looking after it: {error}");
                supervisor.record(&unit, UnitState::new(State::Failed, reason), None);
            }
            // A unit no longer looked after is a change to whoever waits on
            // it to settle.
            ends.store(true, Ordering::SeqCst);
            supervisor.changes.send_modify(|count| *count += 1);
        });
        tending.insert(planned.name.clone(), UnitTask { ended, stop });
    }

    /// Stops `unit`, which runs where `runs` says, and whatever it started,
    /// and waits until nothing of it is left; why not, when something is:
    /// through the task that looks after it, if one does, and otherwise
    /// from its place alone, whatever its settings, as for a unit the plan
    /// does not have.
    pub async fn stop(&self, unit: &str, runs: &Runs) -> Result<(), String> {
        let tended = self
            .tending
            .lock()
            .expect("the tasks are not poisoned")
            .get(unit)
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
        stop_where_it_runs(runs, &self.place(unit)).await
    }

    /// Stops `planned` through the task that looks after it, so that it can
    /// be started afresh, and waits until that task has ended.
    pub async fn stop_to_replace(&self, planned: &PlannedUnit) -> Result<(), String> {
        let Some(runs) = runs(planned) else {
            return Ok(());
        };
        let mut changes = self.changes();
        self.stop(&planned.name, runs).await?;
        // The task ends just after it has answered.
        while self.is_tended(&planned.name) {
            if changes.changed().await.is_err() {
                break;
            }
        }
        Ok(())
    }

    /// The units of which anything is left, found where units run, whether
    /// or not the plan has them.
    pub async fn found_left(&self) -> Result<Vec<(String, Runs)>, String> {
        module::found_left(&self.environment)
            .await
            .map_err(|failure| failure.to_string())
    }

    /// Whether anything of `unit`, which runs where `runs` says, is left
    /// now, from its place alone, whatever its settings.
    pub async fn anything_left(&self, unit: &str, runs: &Runs) -> Result<bool, String> {
        module::anything_left(runs, &self.place(unit))
            .await
            .map_err(|failure| failure.to_string())
    }

    /// Removes what the environment's units share where they run, once
    /// they are stopped; what could not be, by its name, and why.
    pub async fn remove_shared(&self) -> Result<(), (String, String)> {
        module::remove_shared(&self.environment)
            .await
            .map_err(|left| (left.what, left.why.to_string()))
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
pub fn runs(planned: &PlannedUnit) -> Option<&Runs> {
    match &planned.mechanism.as_ref()?.kind {
        UnitKind::Runs(runs) => Some(runs),
        UnitKind::Made => None,
    }
}

/// Stops what of a unit is left where `runs` says it runs, from its place
/// alone.
async fn stop_where_it_runs(runs: &Runs, place: &UnitPlace) -> Result<(), String> {
    module::stop(runs, place)
        .await
        .map_err(|failure| failure.to_string())
}
