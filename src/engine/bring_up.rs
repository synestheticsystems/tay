//! The loop that brings a plan up: it starts each unit once everything it
//! needs is ready, as many at once as their needs allow, reports each change
//! of state, and says as soon as it is decided whether everything asked for
//! is ready or one of those can no longer become ready.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinSet;

use super::fill::{self, UnitValues};
use super::settled::{Settled, Standing};
use super::state::{State, UnitState};
use crate::disk::Environment;
use crate::module::{Exists, Operations, UnitKind, UnitPlace, Values};
use crate::plan::PlannedUnit;

/// What happens while a plan is brought up.
pub enum Progress {
    /// A unit took a state; one that is made and ready gives `values`.
    Changed {
        unit: String,
        state: UnitState,
        values: Option<Values>,
    },
    /// The decision is made. Units that are not waiting on what failed
    /// are still brought up as far as they can be.
    Decided(Decision),
}

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

/// A unit that failed, why, and every unit that was waiting on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedUnit {
    pub unit: String,
    pub reason: String,
    pub waiting: Vec<String>,
}

/// Brings up `units`, in their order, until each has settled or `cancel`
/// says to stop. `asked` are the units the decision is judged by. Each
/// change of state, and the decision once it is made, goes to `progress`.
/// Gives the decision; `None` when it was stopped before it was made.
pub async fn bring_up(
    units: &[Arc<PlannedUnit>],
    asked: &[String],
    environment: &Arc<Environment>,
    progress: &mut (dyn FnMut(Progress) + Send),
    mut cancel: watch::Receiver<bool>,
) -> Option<Decision> {
    let mut bringing = Bringing {
        units,
        progress,
        reported: vec![None; units.len()],
        started: vec![false; units.len()],
        settled: Settled::default(),
        given: UnitValues::new(),
    };
    let mut acting = JoinSet::new();
    // The unit each operation under way acts on, by its task.
    let mut acting_on = HashMap::new();
    let mut decided = None;
    // A change can be cancelled before it begins, while it is planned.
    let mut cancelled = *cancel.borrow();
    loop {
        cancelled |= *cancel.borrow();
        if !cancelled {
            for index in bringing.startable() {
                let planned = Arc::clone(&units[index]);
                let environment = Arc::clone(environment);
                let given = bringing.given.clone();
                let task = acting.spawn_blocking(move || act(&planned, &environment, &given));
                acting_on.insert(task.id(), index);
            }
        }
        // A change cancelled before it was decided stays undecided, though
        // what was under way when it was cancelled ends.
        if decided.is_none() && !cancelled {
            decided = bringing.decided(asked);
            if let Some(decision) = &decided {
                (bringing.progress)(Progress::Decided(decision.clone()));
            }
        }
        if acting.is_empty() {
            break;
        }
        tokio::select! {
            // A cancel that comes as an operation ends is seen first, so
            // nothing more starts after it.
            biased;
            () = until_cancelled(&mut cancel), if !cancelled => cancelled = true,
            Some(joined) = acting.join_next_with_id() => {
                let (task, acted) = match joined {
                    Ok((task, acted)) => (task, acted),
                    // An operation that panicked is a fault in tay, which
                    // fails its unit rather than the daemon.
                    Err(error) => (error.id(), Err(format!("tay failed while acting on it: {error}"))),
                };
                if let Some(index) = acting_on.remove(&task) {
                    bringing.acted(index, acted);
                }
            }
        }
    }
    decided
}

/// Waits until `cancel` says to stop; for ever, once nothing can say so.
async fn until_cancelled(cancel: &mut watch::Receiver<bool>) {
    if cancel.wait_for(|cancelled| *cancelled).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// One plan being brought up.
struct Bringing<'a> {
    units: &'a [Arc<PlannedUnit>],
    progress: &'a mut (dyn FnMut(Progress) + Send),
    /// The state each unit was last reported in.
    reported: Vec<Option<UnitState>>,
    started: Vec<bool>,
    settled: Settled,
    given: UnitValues,
}

impl Bringing<'_> {
    /// The units that may start now. A group becomes ready at once, and a
    /// unit that cannot start yet is reported waiting, or blocked.
    fn startable(&mut self) -> Vec<usize> {
        let mut startable = Vec::new();
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
                if let Some(waiting) = standing.waiting() {
                    self.report(index, waiting, None);
                    continue;
                }
                self.started[index] = true;
                moved = true;
                match &planned.mechanism {
                    None => self.report(
                        index,
                        UnitState::new(State::Ready, "everything it needs is ready"),
                        None,
                    ),
                    Some(mechanism) => {
                        let doing = match mechanism.kind {
                            UnitKind::Made => "making it",
                            UnitKind::Runs(_) => "attempt 1, starting",
                        };
                        self.report(index, UnitState::new(State::Starting, doing), None);
                        startable.push(index);
                    }
                }
            }
        }
        startable
    }

    fn acted(&mut self, index: usize, acted: Result<Exists, String>) {
        match acted {
            Ok(exists) => {
                let name = self.units[index].name.clone();
                self.given.insert(name, exists.values.clone());
                self.report(
                    index,
                    UnitState::new(State::Ready, exists.detail),
                    Some(exists.values),
                );
            }
            Err(reason) => self.report(index, UnitState::new(State::Failed, reason), None),
        }
    }

    fn report(&mut self, index: usize, state: UnitState, values: Option<Values>) {
        let unit = &self.units[index].name;
        if state.state != State::Waiting {
            self.settled.record(unit, state.state);
        }
        if self.reported[index].as_ref() == Some(&state) {
            return;
        }
        self.reported[index] = Some(state.clone());
        (self.progress)(Progress::Changed {
            unit: unit.clone(),
            state,
            values,
        });
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
            .zip(&self.reported)
            .filter(|(planned, _)| failed.contains(&planned.name))
            .filter_map(|(planned, reported)| Some((planned, reported.as_ref()?)))
            .map(|(planned, failure)| FailedUnit {
                unit: planned.name.clone(),
                reason: failure.detail.clone(),
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

/// Makes or starts the unit, with its settings filled from what the units
/// it needs gave. The reason, when it cannot be.
fn act(
    planned: &PlannedUnit,
    environment: &Environment,
    given: &UnitValues,
) -> Result<Exists, String> {
    let mechanism = planned
        .mechanism
        .as_ref()
        .expect("a group is not acted on: it is ready once its needs are");
    let operations = mechanism
        .settings
        .fill(&mut |placeholder| fill::fill(placeholder, environment, given))
        .map_err(|why| why.to_string())?;
    let place = UnitPlace {
        unit: &planned.name,
        environment,
    };
    match operations {
        Operations::Made(made) => made.make(&place).map_err(|failure| failure.to_string()),
        Operations::Running(running) => match running.start(&place) {
            Ok(attempt) => match attempt {},
            Err(failure) => Err(failure.to_string()),
        },
    }
}
