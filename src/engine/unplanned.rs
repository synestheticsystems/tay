//! Units the plan no longer has. A unit that is running and is not in the
//! plan any more is stopped at the next `tay up`, which says so, and
//! `tay status` shows it until then, whatever its module. Such a unit is
//! one the plan left as it changed, or one of which a daemon finds
//! something left where units run that its plan does not list, as it does
//! when the definitions changed while no daemon ran. A container of such a
//! unit that has ended is left too, and goes the same way.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::task::JoinSet;

use super::check::CheckedState;
use super::state::{State, UnitState};
use super::supervisor::Supervisor;
use crate::module::Runs;

/// What a unit the plan does not have is while something of it is left: it
/// cannot become ready, and the change that ends it is the next `tay up`.
const STILL_LEFT: &str =
    "it is not in the plan any more, and something of it is left: the next `tay up` stops it";

/// Sets apart, as unplanned, each unit of which anything is left, found
/// where units run, that the table does not have.
pub async fn find_unplanned(supervisor: &Supervisor) -> Result<(), String> {
    for (unit, runs) in supervisor.found_left().await? {
        supervisor.take_in_unplanned(&unit, runs, UnitState::new(State::Failed, STILL_LEFT));
    }
    Ok(())
}

/// Stops every unit the plan does not have of which something is left, each
/// recorded `stopped` and saying why, and forgets each once nothing of it
/// is left. One that cannot be stopped stays, `failed`, saying why.
pub async fn stop_unplanned(supervisor: &Arc<Supervisor>) {
    let mut stopping = JoinSet::new();
    // The unit each task stops, by its task.
    let mut stopping_of = HashMap::new();
    for unplanned in supervisor.snapshot().each_unplanned() {
        let supervisor = Arc::clone(supervisor);
        let (unit, runs) = (unplanned.name.clone(), unplanned.runs.clone());
        let task = stopping.spawn(async move {
            let stopped = match anything_left(&supervisor, &unit, &runs).await {
                Ok(false) => {
                    supervisor.forget(&unit);
                    return;
                }
                Ok(true) => supervisor.stop(&unit, &runs).await,
                Err(why) => Err(format!(
                    "whether anything of it is left cannot be told: {why}"
                )),
            };
            let state = match &stopped {
                Ok(()) => UnitState::new(
                    State::Stopped,
                    "it is not in the plan any more, so it was stopped",
                ),
                Err(why) => UnitState::new(
                    State::Failed,
                    format!("it is not in the plan any more, and could not be stopped: {why}"),
                ),
            };
            supervisor.record(&unit, state, None);
            if stopped.is_ok() {
                supervisor.forget(&unit);
            }
        });
        stopping_of.insert(task.id(), unplanned.name.clone());
    }
    while let Some(joined) = stopping.join_next_with_id().await {
        // A task that panicked is a fault in tay, which fails the unit it
        // was stopping rather than the change.
        if let Err(error) = joined
            && let Some(unit) = stopping_of.get(&error.id())
        {
            let reason = format!("tay failed while stopping it: {error}");
            supervisor.record(unit, UnitState::new(State::Failed, reason), None);
        }
    }
}

/// Each unit the plan does not have, as a check made now finds it: one of
/// which something is left is as it was recorded, and one of which nothing
/// is left any more is forgotten.
pub async fn check_unplanned(supervisor: &Supervisor) -> Vec<CheckedState> {
    let mut checked = Vec::new();
    for unplanned in supervisor.snapshot().each_unplanned() {
        let state = match anything_left(supervisor, &unplanned.name, &unplanned.runs).await {
            Ok(false) => {
                supervisor.forget(&unplanned.name);
                continue;
            }
            Ok(true) => unplanned.state.clone(),
            Err(why) => UnitState::new(
                State::Failed,
                format!(
                    "it is not in the plan any more, and whether anything of it is left cannot be told: {why}"
                ),
            ),
        };
        checked.push(CheckedState {
            unit: unplanned.name.clone(),
            state,
        });
    }
    checked
}

/// Whether anything of `unit`, which runs where `runs` says, is left now: a
/// task looks after it, or something of it is found where it runs.
async fn anything_left(supervisor: &Supervisor, unit: &str, runs: &Runs) -> Result<bool, String> {
    match supervisor.is_tended(unit) {
        true => Ok(true),
        false => supervisor.anything_left(unit, runs).await,
    }
}
