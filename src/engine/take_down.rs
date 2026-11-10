//! Taking an environment down: every unit is stopped, in the reverse of the
//! order the units start in, then whatever runs of a unit the plan does not
//! have, and then each that runs is checked, so that `tay down` says nothing
//! is left only once it has seen so. A unit that runs is stopped whatever
//! its state, from its place alone if no task looks after it; one that is
//! made is left as it is on disk, to be had again at the next `tay up`, and
//! is `stopped` while the environment is down.

use std::sync::Arc;

use super::state::{State, UnitState};
use super::supervisor::Supervisor;
use crate::module::UnitKind;

/// A unit that could not be stopped, or of which something was left, and
/// why.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Left {
    pub unit: String,
    pub reason: String,
}

/// What is done with what is found to run of a unit the plan does not
/// have.
#[derive(Clone, Copy)]
enum Pass {
    Stop,
    Check,
}

/// Stops each unit `supervisor` knows, recording each as stopped, with
/// `detail` saying why, and checks that nothing of any is left; what could
/// not be stopped, or was left, is recorded `failed` and given back. Why
/// what runs cannot be told, when it cannot.
pub async fn take_down(supervisor: &Arc<Supervisor>, detail: &str) -> Result<Vec<Left>, String> {
    let units = supervisor.snapshot();
    let mut left = Vec::new();
    for known in units.iter().rev() {
        let planned = &known.planned;
        let runs = matches!(
            planned.mechanism.as_ref().map(|mechanism| &mechanism.kind),
            Some(UnitKind::Runs(_))
        );
        let stopped = match runs {
            true => supervisor.stop(planned).await,
            false => Ok(()),
        };
        let state = match stopped {
            Ok(()) => UnitState::new(State::Stopped, detail),
            Err(reason) => {
                left.push(Left {
                    unit: planned.name.clone(),
                    reason: reason.clone(),
                });
                UnitState::new(State::Failed, format!("could not be stopped: {reason}"))
            }
        };
        supervisor.record(&planned.name, state, None);
    }
    // What runs of a unit the plan does not have, as one left from before
    // a change of what was asked for, is stopped from its place alone, and
    // then looked for again.
    let known = |unit: &str| units.iter().any(|known| known.planned.name == unit);
    for pass in [Pass::Stop, Pass::Check] {
        let found = supervisor
            .found_running()
            .map_err(|why| format!("what runs of the environment's units cannot be told: {why}"))?;
        for (unit, runs) in found {
            if known(&unit) || left.iter().any(|left| left.unit == unit) {
                continue;
            }
            let stopped = match pass {
                Pass::Stop => supervisor.stop_unplanned(&unit, runs).await,
                Pass::Check => Err("something of it still runs after it was stopped".to_string()),
            };
            if let Err(reason) = stopped {
                left.push(Left { unit, reason });
            }
        }
    }
    for known in units.iter() {
        let planned = &known.planned;
        if left.iter().any(|left| left.unit == planned.name) {
            continue;
        }
        let reason = match supervisor.running_now(planned).await {
            Ok(false) => continue,
            Ok(true) => "something of it still runs after it was stopped".to_string(),
            Err(why) => format!("whether anything of it still runs cannot be told: {why}"),
        };
        supervisor.record(
            &planned.name,
            UnitState::new(State::Failed, reason.clone()),
            None,
        );
        left.push(Left {
            unit: planned.name.clone(),
            reason,
        });
    }
    Ok(left)
}
