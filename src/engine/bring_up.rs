//! The loop that brings a plan up: it starts each unit once everything it
//! needs is ready, as many at once as their needs allow, records each change
//! of state, and says as soon as it is decided whether everything asked for
//! is ready or one of those can no longer become ready.
//!
//! A unit that is made is made on the blocking pool. A unit that runs is
//! handed to the supervisor, whose task looks after it from then on, and the
//! loop waits for it to become ready or fail: an attempt that fails and is
//! followed by one that becomes ready counts as ready. A unit it waits for
//! while it is `unready` is waited for no longer than its own `within`,
//! counted from when the loop first sees it so, and is then lost to the
//! bring-up as a failed one is, though it goes on running and being
//! checked. What a unit that runs already would be started from now is
//! compared with what it was started from; if they differ it is started
//! again, saying what changed, and otherwise joined as it is, its state
//! said once as it is. A bring-up that leaves what is up as it is joins it
//! unasked. A bring-up for `tay up` has what the plan no longer
//! has leave first, as [`Leaving`] says: until it has, only the units it
//! waits for start, and nothing is decided. Each unit it starts is timed,
//! and the decision is told with how long each took.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::Instant;

use super::leaving::{Leaving, NotUndone, Step};
use super::settled::{Settled, Standing};
use super::started_from;
use super::state::{State, UnitState};
use super::supervisor::{Begin, Supervisor};
use super::units::Took;
use crate::module::{Exists, Made, Operations, Running};
use crate::plan::{PlannedUnit, Readiness};

/// What bringing a plan up comes to for the units it was asked for, and
/// for `tay up` for the `undo`s owed, decided as soon as it can be.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "lowercase")]
pub enum Decision {
    /// Every unit asked for is ready, and every `undo` owed has run.
    Ready,
    /// A unit asked for can no longer become ready, or an `undo` owed did
    /// not run, because these failed.
    Failed { failed: Vec<FailedUnit> },
}

/// A unit that failed, stayed unready, or whose `undo` did not run, the
/// state it is in, why, the last lines it wrote, and every unit that was
/// waiting on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedUnit {
    pub unit: String,
    pub state: State,
    pub reason: String,
    pub lines: Vec<String>,
    pub waiting: Vec<String>,
}

/// What bringing a plan up does with a unit that runs already and would
/// be started from something else now.
#[derive(Clone, Copy)]
pub enum Outdated {
    /// It is started again, as `tay up` does.
    Replace,
    /// It is joined as it runs, as `tay run` does, which never replaces a
    /// unit that is up.
    Join,
}

/// What a bring-up tells the change that made it, as it goes.
pub enum Telling {
    /// What it came to, once that is decided, and how long each unit it
    /// started took until then, the longest first.
    Decided(Decision, Vec<Took>),
    /// Something a person should know of it, said once.
    Notice(String),
}

/// Brings up `units`, in their order, until each has settled or `cancel`
/// says to stop, doing with a unit that runs from other settings what
/// `outdated` says, and first, with `leaving`, what it says. `asked` are
/// the units the decision is judged by, which `told` is told once it is
/// made, as it is each notice. Gives the decision; `None` when it was
/// stopped before it was made.
pub async fn bring_up(
    supervisor: &Arc<Supervisor>,
    units: &[Arc<PlannedUnit>],
    asked: &[String],
    outdated: Outdated,
    leaving: Option<Leaving>,
    told: &mut (dyn FnMut(Telling) + Send),
    mut cancel: watch::Receiver<bool>,
) -> Option<Decision> {
    supervisor.time_afresh();
    let mut bringing = Bringing {
        supervisor,
        units,
        outdated,
        leaving,
        started: vec![false; units.len()],
        settled: Settled::default(),
        running: HashSet::new(),
        handed: false,
        joined: HashSet::new(),
        unready_since: HashMap::new(),
    };
    let mut making = JoinSet::new();
    // The unit each one being made is, by its task.
    let mut making_of = HashMap::new();
    // What runs already, compared with what it was started from, and
    // started again if that changed.
    let mut comparing = JoinSet::new();
    let mut comparing_of = HashMap::new();
    // The step of leaving under way, one at a time.
    let mut leaving_step = JoinSet::new();
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
                if leaving_step.is_empty()
                    && let Some(step) = bringing.leave()
                {
                    leaving_step.spawn(step);
                    moved = true;
                }
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
                told(Telling::Decided(decision.clone(), supervisor.took()));
            }
        }
        // What runs goes on under the supervisor whatever happens here.
        let waits_on_running = !bringing.running.is_empty() && !cancelled;
        let idle = making.is_empty() && comparing.is_empty() && leaving_step.is_empty();
        if idle && !waits_on_running {
            break;
        }
        let unready_until = bringing.unready_until().filter(|_| waits_on_running);
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
                    Err(error) => (error.id(), Compared::Unknown(format!("tay failed while comparing what it was started from: {error}"))),
                };
                if let Some(index) = comparing_of.remove(&task) {
                    if let Compared::Unknown(why) = &compared {
                        told(Telling::Notice(format!(
                            "whether `{}` would be started from what it was started from cannot be told, so it is left as it runs: {why}",
                            units[index].name
                        )));
                    }
                    bringing.compared(index, compared);
                }
            }
            Some(joined) = leaving_step.join_next() => {
                // A step that panicked is a fault in tay, which is said
                // rather than left to hold up the rest.
                let came = joined.unwrap_or_else(|error| {
                    told(Telling::Notice(format!("tay failed while undoing or stopping what the plan no longer has: {error}")));
                    None
                });
                bringing.left(came);
            }
            changed = changes.changed(), if waits_on_running => if changed.is_err() {
                break;
            },
            // A unit waited for while unready is given up on once its
            // `within` has passed, whether or not anything changed.
            () = tokio::time::sleep_until(unready_until.unwrap_or_else(Instant::now)), if unready_until.is_some() => {}
        }
    }
    decision
}

/// What became of a unit that ran already when it was brought up.
enum Compared {
    /// It would be started from what it was started from, and is joined
    /// as it is.
    Current,
    /// It would be started from something else, and was started afresh.
    Replaced,
    /// It would be started from something else, and could not be stopped,
    /// as this says.
    NotStopped(String),
    /// What it would be started from, or what it was started from, cannot
    /// be told, as this says, so it is joined as it is.
    Unknown(String),
}

/// Compares what `planned`, which runs already, would be started from now,
/// with `running`, and what it was started from, and starts it again if
/// they differ, unless the change is cancelled first.
async fn compared(
    supervisor: Arc<Supervisor>,
    planned: Arc<PlannedUnit>,
    running: Arc<dyn Running>,
    cancel: watch::Receiver<bool>,
) -> Compared {
    let why = match started_from::changed(&supervisor, &planned, Arc::clone(&running)).await {
        Ok(None) => return Compared::Current,
        Ok(Some(why)) => why,
        Err(why) => return Compared::Unknown(why),
    };
    // Its time runs from here: stopping what runs of it is part of
    // starting it again.
    let again = UnitState::new(State::Starting, format!("{why}, so it is started again"));
    supervisor.time(&planned.name);
    supervisor.record(&planned.name, again, None);
    replace(&supervisor, &planned, running, &cancel).await
}

/// Stops `planned`, which would be started from something else now, and
/// starts it afresh with `running`, unless the change is cancelled first.
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
    outdated: Outdated,
    /// What is done first with what the plan no longer has, for `tay up`.
    leaving: Option<Leaving>,
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
    /// When the loop first saw each unit it waits for `unready`, while it
    /// is.
    unready_since: HashMap<usize, Instant>,
}

/// What a pass over the units started: the units that are made, to be made,
/// each with whether what uses it runs, and the units that run already,
/// with the operations the plan now gives them, to be compared with what
/// they were started from.
#[derive(Default)]
struct Started {
    made: Vec<(usize, Box<dyn Made>, bool)>,
    tended: Vec<(usize, Arc<dyn Running>)>,
}

impl Bringing<'_> {
    /// Starts the units that may start now, and gives those that are made,
    /// to be made, and those that run already, to be compared. A group
    /// becomes ready at once, a unit that runs is handed to the supervisor,
    /// and a unit that cannot start yet is recorded waiting, or blocked. A
    /// unit made or handed over is timed from now; a group starts nothing,
    /// and is not.
    fn start(&mut self) -> Started {
        let mut started = Started::default();
        // A group made ready can clear the units after it.
        let mut moved = true;
        while moved {
            moved = false;
            for index in 0..self.units.len() {
                let held = (self.leaving.as_ref())
                    .is_some_and(|leaving| leaving.holds(&self.units[index].name));
                if self.started[index] || held {
                    continue;
                }
                let planned = Arc::clone(&self.units[index]);
                let standing = self.settled.standing(&planned);
                if let Standing::Blocked(failed) = &standing {
                    self.settled.blocked(&planned.name, failed.clone());
                }
                // A unit that runs already is compared once what it needs
                // has settled, so that how those came out is said: it is
                // joined as it is, its state its own, unless what it would
                // be started from changed.
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
                    // gives it, unless it is to be joined whatever they are;
                    // one whose settings cannot be filled now is joined as
                    // it runs.
                    Ok(Operations::Running(running))
                        if tended && matches!(self.outdated, Outdated::Replace) =>
                    {
                        started.tended.push((index, running));
                    }
                    _ if tended => self.join(index),
                    Err(why) => self.record(index, UnitState::new(State::Failed, why.to_string())),
                    Ok(Operations::Made(operations)) => {
                        self.supervisor.time(&planned.name);
                        self.record(index, UnitState::new(State::Starting, "making it"));
                        started.made.push((
                            index,
                            operations,
                            self.supervisor.in_use(&planned.name),
                        ));
                    }
                    Ok(Operations::Running(running)) => {
                        self.supervisor.time(&planned.name);
                        self.supervisor.tend(&planned, running, Begin::Fresh, true);
                        self.running.insert(index);
                        self.handed = true;
                    }
                }
            }
        }
        started
    }

    /// The next step of leaving, as the units stand, once the one before it
    /// has ended; `None` while none can begin, and for a bring-up that has
    /// nothing leave.
    fn leave(&mut self) -> Option<Step> {
        self.leaving.as_mut()?.next(self.supervisor, &self.settled)
    }

    /// The step of leaving under way has ended, as `came` says of an
    /// `undo` that did not run.
    fn left(&mut self, came: Option<NotUndone>) {
        if let Some(leaving) = &mut self.leaving {
            leaving.ended(came);
        }
    }

    /// Joins the unit at `index`, which runs already, as it is: nothing
    /// starts it again, and its state is its own, which is said now, as no
    /// change of it would say it.
    fn join(&mut self, index: usize) {
        self.supervisor.say_again(&self.units[index].name);
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
                    "what it would be started from has changed, and what runs of it could not be stopped to start it again: {why}"
                );
                self.record(index, UnitState::new(State::Failed, reason));
            }
        }
    }

    /// Settles each unit that runs that has become ready or failed, that
    /// has been `unready` longer than its own `within`, or that can no
    /// longer start because a need of it is lost; whether any settled. A
    /// unit this change found running and joined, whose task has ended with
    /// it not ready, is started afresh, once: it is not running now, and
    /// `tay up` brings up what is not.
    fn settle_running(&mut self) -> bool {
        let before = self.running.len();
        let running: Vec<usize> = self.running.iter().copied().collect();
        for index in running {
            let planned = Arc::clone(&self.units[index]);
            let state = self
                .supervisor
                .state(&planned.name)
                .map(|state| state.state);
            if state != Some(State::Unready) {
                self.unready_since.remove(&index);
            }
            let joined = self.joined.contains(&index);
            match state {
                Some(State::Ready) => self.settled.record(&planned.name, State::Ready),
                Some(State::Failed) if !joined => self.settled.record(&planned.name, State::Failed),
                Some(State::Unready) if self.unready_too_long(index) => {
                    self.settled.stayed_unready(&planned.name);
                }
                _ if self.supervisor.is_tended(&planned.name) => continue,
                _ if joined => {
                    self.joined.remove(&index);
                    self.started[index] = false;
                }
                // Its task ended without it ready or failed: a need of it
                // failed before it could start again.
                _ => match self.settled.standing(&planned) {
                    Standing::Blocked(failed) => self.settled.blocked(&planned.name, failed),
                    Standing::Clear | Standing::Waiting(_) => {
                        self.settled.record(&planned.name, State::Failed);
                    }
                },
            }
            self.running.remove(&index);
            self.unready_since.remove(&index);
        }
        self.running.len() != before
    }

    /// Whether the unit at `index`, which is `unready`, has been so longer
    /// than its own `within` since the loop first saw it so; it is timed
    /// from now if it was not.
    fn unready_too_long(&mut self, index: usize) -> bool {
        let since = *self.unready_since.entry(index).or_insert_with(Instant::now);
        since.elapsed() >= unready_allowance(&self.units[index])
    }

    /// When the first unit the loop waits for while it is `unready` will
    /// have been so longer than its own `within`.
    fn unready_until(&self) -> Option<Instant> {
        (self.unready_since.iter())
            .map(|(index, since)| *since + unready_allowance(&self.units[*index]))
            .min()
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

    /// The decision, once it can be made: once what the plan no longer has
    /// has left, too, an `undo` of which that did not run failing it.
    fn decided(&self, asked: &[String]) -> Option<Decision> {
        let not_undone = match &self.leaving {
            Some(leaving) if !leaving.is_over() => return None,
            Some(leaving) => leaving.not_undone(),
            None => &[],
        };
        let ready = asked.iter().all(|unit| self.settled.is_ready(unit));
        if !ready && !asked.iter().any(|unit| self.settled.is_lost(unit)) {
            return None;
        }
        let mut failed = match ready {
            true => Vec::new(),
            false => self.failed_under(asked),
        };
        failed.extend(not_undone.iter().map(|not_undone| FailedUnit {
            unit: not_undone.unit.clone(),
            state: State::Failed,
            reason: not_undone.reason.clone(),
            lines: match not_undone.ran {
                true => self.supervisor.last_lines(&not_undone.unit),
                false => Vec::new(),
            },
            waiting: Vec::new(),
        }));
        Some(match ready && failed.is_empty() {
            true => Decision::Ready,
            false => Decision::Failed { failed },
        })
    }

    /// Each unit that failed that one of `asked` waits on, or that is one.
    fn failed_under(&self, asked: &[String]) -> Vec<FailedUnit> {
        let failed: BTreeSet<String> = asked
            .iter()
            .flat_map(|unit| self.settled.failures_under(unit))
            .collect();
        self.units
            .iter()
            .filter(|planned| failed.contains(&planned.name))
            .filter_map(|planned| Some((planned, self.supervisor.state(&planned.name)?)))
            .map(|(planned, failure)| FailedUnit {
                unit: planned.name.clone(),
                state: match self.settled.stayed_unready_is(&planned.name) {
                    true => State::Unready,
                    false => State::Failed,
                },
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
            .collect()
    }
}

/// How long a bring-up waits for `planned` while it is `unready`: its own
/// `within`. A unit that writes no `ready` has its module judge it, as by a
/// container's own health check, which has allowed for it already.
fn unready_allowance(planned: &PlannedUnit) -> Duration {
    match planned.mechanism.as_ref().map(|mechanism| &mechanism.ready) {
        Some(Readiness::Check { within, .. }) => within.length(),
        _ => Duration::ZERO,
    }
}
