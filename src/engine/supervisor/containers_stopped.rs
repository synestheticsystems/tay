//! What runs containers stopping under the units, as Docker does when it
//! is upgraded or restarted: every container ends with it, and none of that
//! is a unit's doing. Each unit in a container is let go, rather than have
//! what ends counted against it, and waits; the bring-up once it answers
//! again starts each afresh. Host processes are not touched.

use super::{Supervisor, UnitTask, runs};
use crate::clock;
use crate::engine::state::{State, UnitState};
use crate::module::{self, Runs};
use crate::plan::PlannedUnit;

impl Supervisor {
    /// Whether a unit of the plan runs in a container.
    pub fn runs_in_a_container(&self) -> bool {
        self.units()
            .iter()
            .any(|known| in_a_container(&known.planned))
    }

    /// What runs containers stopped answering at `at`, in seconds since
    /// 1970, for the reason `why` gives. Each unit in a container that a
    /// task looks after is let go, whatever is left of it left where it
    /// is, and waits; so does one in a container that failed or was
    /// starting, whose end may have been of this. One that has done what
    /// it does, as a command has, stays as it is.
    pub fn containers_stopped_answering(&self, at: u64, why: &str) {
        let waiting = format!(
            "for {}, which stopped answering at {}: {why}",
            module::CONTAINERS_RUN_BY,
            clock::utc(at)
        );
        let snapshot = self.snapshot();
        let in_containers = snapshot
            .iter()
            .filter(|known| in_a_container(&known.planned));
        for known in in_containers {
            let let_go = self.let_go(&known.planned.name);
            let unsettled = matches!(
                known.state.state,
                State::Failed | State::Starting | State::Unready
            );
            if let_go || unsettled {
                let state = UnitState::new(State::Waiting, waiting.clone());
                self.record(&known.planned.name, state, None);
            }
        }
    }

    /// Stops looking after `unit`, leaving whatever of it runs where it
    /// runs; whether a task looked after it.
    fn let_go(&self, unit: &str) -> bool {
        let tending = self.tending.lock().expect("the tasks are not poisoned");
        match tending.get(unit).filter(|task| UnitTask::is_live(task)) {
            Some(task) => {
                task.abort.abort();
                true
            }
            None => false,
        }
    }
}

/// Whether `planned` runs in a container.
fn in_a_container(planned: &PlannedUnit) -> bool {
    runs(planned).is_some_and(Runs::in_a_container)
}
