//! The loop that brings a plan up: it starts each unit once everything it
//! needs is ready, as many at once as their needs allow, records each change
//! of state, and says as soon as it is decided whether everything asked for
//! is ready or one of those can no longer become ready.
//!
//! A unit that is made is made on the blocking pool. A unit that runs is
//! handed to the supervisor, whose task looks after it from then on, and the
//! loop waits for it to become ready or fail: an attempt that fails and is
//! followed by one that becomes ready counts as ready. A unit that runs
//! already is asked whether what runs of it was started from other settings
//! than the plan now gives; if it was, it is replaced, and otherwise joined
//! as it is.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinSet;

use super::settled::{Settled, Standing};
use super::state::{State, UnitState};
use super::supervisor::{Begin, Supervisor};
use crate::module::{Exists, Made, Operations, Running, UnitKind};
use crate::plan::PlannedUnit;

/// What bringing a plan up comes to for the units it was asked for,
/// decided as soon as it can be.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "lowercase")]
pub enum Decision {
    /// Every unit asked for is ready.
    Ready,
    /// A unit asked for can no longer become ready, because these failed.
    Failed { failed: Vec<FailedUnit> },
}

/// A unit that failed, why, the last lines it wrote, and every unit that
/// was waiting on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedUnit {
    pub unit: String,
    pub reason: String,
    pub lines: Vec<String>,
    pub waiting: Vec<String>,
}

/// Brings up `units`, in their order, until each has settled or `cancel`
/// says to stop. `asked` are the units the decision is judged by, which goes
/// to `decided` once it is made. Gives the decision; `None` when it was
/// stopped before it was made.
pub async fn bring_up(
    supervisor: &Arc<Supervisor>,
    units: &[Arc<PlannedUnit>],
    asked: &[String],
    decided: &mut (dyn FnMut(Decision) + Send),
    noticed: &mut (dyn FnMut(String) + Send),
    mut cancel: watch::Receiver<bool>,
) -> Option<Decision> {
    let mut bringing = Bringing {
        supervisor,
        units,
        started: vec![false; units.len()],
        settled: Settled::default(),
        running: HashSet::new(),
        handed: false,
        joined: HashSet::new(),
    };
    let mut making = JoinSet::new();
    // The unit each one being made is, by its task.
    let mut making_of = HashMap::new();
    // What runs already, its settings being compared with the plan's, and
    // replaced if they changed.
    let mut comparing = JoinSet::new();
    let mut comparing_of = HashMap::new();
    let mut changes = supervisor.changes();
    let mut decision = None;
    // A change can be cancelled before it begins, while it is planned.
    let mut cancelled = *cancel.borrow();
    loop {
        cancelled |= *cancel.borrow();
        // A unit that settles clears those that need it to start, and one
        // that starts may be ready already, as one found again is.
        let mut moved = true;
        while moved {
            moved = bringing.settle_running();
            if !cancelled {
                let started = bringing.start();
                for (index, made, in_use) in started.made {
                    moved = true;
                    let supervisor = Arc::clone(supervisor);
                    let unit = units[index].name.clone();
                    let task =
                        making.spawn_blocking(move || made.make(&supervisor.place(&unit), in_use));
                    making_of.insert(task.id(), index);
                }
                for (index, running) in started.tended {
                    moved = true;
                    let task = comparing.spawn(compared(
                        Arc::clone(supervisor),
                        Arc::clone(&units[index]),
                        running,
                        cancel.clone(),
                    ));
                    comparing_of.insert(task.id(), index);
                }
                moved |= bringing.handed;
                bringing.handed = false;
            }
        }
        // A change cancelled before it was decided stays undecided, though
        // what was being made when it was cancelled ends.
        if decision.is_none() && !cancelled {
            decision = bringing.decided(asked);
            if let Some(decision) = &decision {
                decided(decision.clone());
            }
        }
        // What runs goes on under the supervisor whatever happens here.
        let waits_on_running = !bringing.running.is_empty() && !cancelled;
        if making.is_empty() && comparing.is_empty() && !waits_on_running {
            break;
        }
        tokio::select! {
            // A cancel that comes as a unit is made is seen first, so
            // nothing more starts after it.
            biased;
            () = until_cancelled(&mut cancel), if !cancelled => cancelled = true,
            Some(joined) = making.join_next_with_id() => {
                let (task, made) = match joined {
                    Ok((task, made)) => (task, made.map_err(|failure| failure.to_string())),
                    // An operation that panicked is a fault in tay, which
                    // fails its unit rather than the daemon.
                    Err(error) => (error.id(), Err(format!("tay failed while making it: {error}"))),
                };
                if let Some(index) = making_of.remove(&task) {
                    bringing.made(index, made);
                }
            }
            Some(joined) = comparing.join_next_with_id() => {
                let (task, compared) = match joined {
                    Ok((task, compared)) => (task, compared),
                    Err(error) => (error.id(), Compared::Unknown(format!("tay failed while comparing its settings: {error}"))),
                };
                if let Some(index) = comparing_of.remove(&task) {
                    if let Compared::Unknown(why) = &compared {
                        noticed(format!(
                            "whether `{}` runs from the settings the plan now gives cannot be told, so it is left as it runs: {why}",
                            units[index].name
                        ));
                    }
                    bringing.compared(index, compared);
                }
            }
            changed = changes.changed(), if waits_on_running => if changed.is_err() {
                break;
            },
        }
    }
    decision
}

/// What became of a unit that ran already when it was brought up.
enum Compared {
    /// It runs from the settings the plan gives, and is joined as it is.
    Current,
    /// It ran from other settings, and was started afresh from these.
    Replaced,
    /// It ran from other settings, and could not be stopped, as this says.
    NotStopped(String),
    /// Whether it runs from these settings cannot be told, as this says, so
    /// it is joined as it is.
    Unknown(String),
}

/// Asks whether `planned`, which runs already, runs from the settings
/// `running` has, and replaces it if not, unless the change is cancelled
/// first.
async fn compared(
    supervisor: Arc<Supervisor>,
    planned: Arc<PlannedUnit>,
    running: Arc<dyn Running>,
    cancel: watch::Receiver<bool>,
) -> Compared {
    let place = supervisor.place(&planned.name);
    match Arc::clone(&running).outdated(place).await {
        Ok(false) => Compared::Current,
        Err(failure) => Compared::Unknown(failure.to_string()),
        Ok(true) => {
            let replacing = UnitState::new(
                State::Starting,
                "its settings have changed, so what runs of it is replaced",
            );
            supervisor.record(&planned.name, replacing, None);
            replace(&supervisor, &planned, running, &cancel).await
        }
    }
}

/// Stops `planned`, which runs from other settings than `running` has, and
/// starts it afresh from these, unless the change is cancelled first.
async fn replace(
    supervisor: &Arc<Supervisor>,
    planned: &Arc<PlannedUnit>,
    running: Arc<dyn Running>,
    cancel: &watch::Receiver<bool>,
) -> Compared {
    match supervisor.stop_to_replace(planned).await {
        Err(why) => Compared::NotStopped(why),
        // What is stopped for a change that is then cancelled stays
        // stopped.
        Ok(()) if *cancel.borrow() => Compared::Replaced,
        Ok(()) => {
            supervisor.tend(planned, running, Begin::Fresh, true);
            Compared::Replaced
        }
    }
}

/// Waits until `cancel` says to stop; for ever, once nothing can say so.
async fn until_cancelled(cancel: &mut watch::Receiver<bool>) {
    if cancel.wait_for(|cancelled| *cancelled).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// One plan being brought up.
struct Bringing<'a> {
    supervisor: &'a Arc<Supervisor>,
    units: &'a [Arc<PlannedUnit>],
    started: Vec<bool>,
    settled: Settled,
    /// The units that run, handed to the supervisor, not yet ready or
    /// failed.
    running: HashSet<usize>,
    /// Whether a unit that runs has been handed to the supervisor since
    /// this was last cleared.
    handed: bool,
    /// The units that run that this change found running, and joined.
    joined: HashSet<usize>,
}

/// What a pass over the units started: the units that are made, to be made,
/// each with whether what uses it runs, and the units that run already,
/// with the operations the plan now gives them, to have their settings compared.
#[derive(Default)]
struct Started {
    made: Vec<(usize, Box<dyn Made>, bool)>,
    tended: Vec<(usize, Arc<dyn Running>)>,
}

impl Bringing<'_> {
    /// Starts the units that may start now, and gives those that are made,
    /// to be made, and those that run already, to have their settings compared. A group
    /// becomes ready at once, a unit that runs is handed to the supervisor,
    /// and a unit that cannot start yet is recorded waiting, or blocked.
    fn start(&mut self) -> Started {
        let mut started = Started::default();
        // A group made ready can clear the units after it.
        let mut moved = true;
        while moved {
            moved = false;
            for index in 0..self.units.len() {
                if self.started[index] {
                    continue;
                }
                let planned = Arc::clone(&self.units[index]);
                let standing = self.settled.standing(&planned);
                if let Standing::Blocked(failed) = &standing {
                    self.settled.blocked(&planned.name, failed.clone());
                }
                // A unit that runs already has its settings compared once what it needs has
                // settled, so that how those came out is said: it is joined
                // as it is, its state its own, unless its settings changed.
                let tended = self.supervisor.is_tended(&planned.name);
                if let Some(waiting) = standing.waiting() {
                    if !tended {
                        self.record(index, waiting);
                    }
                    continue;
                }
                self.started[index] = true;
                moved = true;
                let Some(mechanism) = &planned.mechanism else {
                    self.record(
                        index,
                        UnitState::new(State::Ready, "everything it needs is ready"),
                    );
                    continue;
                };
                let given = self.supervisor.given();
                let operations = mechanism
                    .operations(&mut |placeholder| self.supervisor.fill(placeholder, &given));
                match operations {
                    // What runs already is compared with the settings the plan
                    // gives it; one whose settings cannot be filled now is
                    // joined as it runs.
                    Ok(Operations::Running(running)) if tended => {
                        started.tended.push((index, running));
                    }
                    _ if tended => self.join(index),
                    Err(why) => self.record(index, UnitState::new(State::Failed, why.to_string())),
                    Ok(Operations::Made(operations)) => {
                        self.record(index, UnitState::new(State::Starting, "making it"));
                        started
                            .made
                            .push((index, operations, self.in_use(&planned.name)));
                    }
                    Ok(Operations::Running(running)) => {
                        self.supervisor.tend(&planned, running, Begin::Fresh, true);
                        self.running.insert(index);
                        self.handed = true;
                    }
                }
            }
        }
        started
    }

    /// Joins the unit at `index`, which runs already, as it is: nothing
    /// starts it again, and its state is its own.
    fn join(&mut self, index: usize) {
        self.running.insert(index);
        self.joined.insert(index);
        self.handed = true;
    }

    /// Takes in what became of the unit at `index`, which ran already.
    fn compared(&mut self, index: usize, compared: Compared) {
        match compared {
            Compared::Current | Compared::Unknown(_) => self.join(index),
            Compared::Replaced => {
                self.running.insert(index);
                self.handed = true;
            }
            Compared::NotStopped(why) => {
                let reason = format!(
                    "its settings have changed, and what runs of it could not be stopped to replace it: {why}"
                );
                self.record(index, UnitState::new(State::Failed, reason));
            }
        }
    }

    /// Whether a unit that runs and uses `unit`'s values runs now, directly
    /// or through units that are made from them, as a process does that
    /// reads a port from a file written with it. A unit that runs is where
    /// the chain stops: what needs it holds nothing of `unit`'s.
    fn in_use(&self, unit: &str) -> bool {
        let made = |name: &str| {
            self.units.iter().any(|planned| {
                planned.name == name
                    && planned
                        .mechanism
                        .as_ref()
                        .is_some_and(|mechanism| matches!(mechanism.kind, UnitKind::Made))
            })
        };
        let mut through = vec![unit.to_string()];
        let mut users = Vec::new();
        while let Some(used) = through.pop() {
            for user in self.units.iter() {
                if !user.needs.iter().any(|need| need.unit == used) || users.contains(&user.name) {
                    continue;
                }
                users.push(user.name.clone());
                if made(&user.name) {
                    through.push(user.name.clone());
                }
            }
        }
        users.iter().any(|user| self.supervisor.is_tended(user))
    }

    /// Settles each unit that runs that has become ready or failed, or can
    /// no longer start because a need of it failed; whether any settled. A
    /// unit this change found running and joined, whose task has ended with
    /// it not ready, is started afresh, once: it is not running now, and
    /// `tay up` brings up what is not.
    fn settle_running(&mut self) -> bool {
        let before = self.running.len();
        let running: Vec<usize> = self.running.iter().copied().collect();
        for index in running {
            let planned = &self.units[index];
            let state = self
                .supervisor
                .state(&planned.name)
                .map(|state| state.state);
            let joined = self.joined.contains(&index);
            match state {
                Some(State::Ready) => self.settled.record(&planned.name, State::Ready),
                Some(State::Failed) if !joined => self.settled.record(&planned.name, State::Failed),
                _ if self.supervisor.is_tended(&planned.name) => continue,
                _ if joined => {
                    self.joined.remove(&index);
                    self.started[index] = false;
                }
                // Its task ended without it ready or failed: a need of it
                // failed before it could start again.
                _ => match self.settled.standing(planned) {
                    Standing::Blocked(failed) => self.settled.blocked(&planned.name, failed),
                    Standing::Clear | Standing::Waiting(_) => {
                        self.settled.record(&planned.name, State::Failed);
                    }
                },
            }
            self.running.remove(&index);
        }
        self.running.len() != before
    }

    fn made(&mut self, index: usize, made: Result<Exists, String>) {
        match made {
            Ok(exists) => {
                let unit = &self.units[index].name;
                self.settled.record(unit, State::Ready);
                self.supervisor.record(
                    unit,
                    UnitState::new(State::Ready, exists.detail),
                    Some(exists.values),
                );
            }
            Err(reason) => self.record(index, UnitState::new(State::Failed, reason)),
        }
    }

    /// Records the state the unit at `index` took, here and with the
    /// supervisor.
    fn record(&mut self, index: usize, state: UnitState) {
        let unit = &self.units[index].name;
        if state.state != State::Waiting {
            self.settled.record(unit, state.state);
        }
        self.supervisor.record(unit, state, None);
    }

    /// The decision, once it can be made.
    fn decided(&self, asked: &[String]) -> Option<Decision> {
        if asked.iter().all(|unit| self.settled.is_ready(unit)) {
            return Some(Decision::Ready);
        }
        if !asked.iter().any(|unit| self.settled.is_lost(unit)) {
            return None;
        }
        let failed: BTreeSet<String> = asked
            .iter()
            .flat_map(|unit| self.settled.failures_under(unit))
            .collect();
        let failed = self
            .units
            .iter()
            .filter(|planned| failed.contains(&planned.name))
            .filter_map(|planned| Some((planned, self.supervisor.state(&planned.name)?)))
            .map(|(planned, failure)| FailedUnit {
                unit: planned.name.clone(),
                reason: failure.detail,
                lines: self.supervisor.last_lines(&planned.name),
                waiting: self
                    .units
                    .iter()
                    .filter(|other| {
                        other.name != planned.name
                            && self
                                .settled
                                .failures_under(&other.name)
                                .contains(&planned.name)
                    })
                    .map(|other| other.name.clone())
                    .collect(),
            })
            .collect();
        Some(Decision::Failed { failed })
    }
}
