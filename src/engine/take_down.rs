//! Taking an environment down: every unit is stopped, in the reverse of the
//! order the units start in. A unit that runs is asked to stop if it may be
//! running; one that is made is left as it is on disk, to be had again at
//! the next `tay up`, and is `stopped` while the environment is down.

use super::fill;
use super::state::{State, UnitState};
use super::units::Units;
use crate::disk::Environment;
use crate::module::{Operations, UnitKind, UnitPlace};

/// A unit that could not be stopped, and why.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Left {
    pub unit: String,
    pub reason: String,
}

/// Stops each of `units`, reporting each change of state to `stopped`,
/// with `detail` saying why; what could not be stopped is reported
/// `failed` and given back.
pub fn take_down(
    units: &Units,
    environment: &Environment,
    detail: &str,
    changed: &mut dyn FnMut(&str, UnitState),
) -> Vec<Left> {
    let given = units.given();
    let mut left = Vec::new();
    for known in units.iter().rev() {
        let planned = &known.planned;
        let may_run = matches!(
            (
                planned.mechanism.as_ref().map(|mechanism| &mechanism.kind),
                known.state.state
            ),
            (Some(UnitKind::Runs(_)), State::Starting | State::Ready)
        );
        let stopped = match &planned.mechanism {
            Some(mechanism) if may_run => {
                let place = UnitPlace {
                    unit: &planned.name,
                    environment,
                };
                mechanism
                    .settings
                    .fill(&mut |placeholder| fill::fill(placeholder, environment, &given))
                    .map_err(|why| why.to_string())
                    .and_then(|operations| match operations {
                        Operations::Running(running) => {
                            running.stop(&place).map_err(|failure| failure.to_string())
                        }
                        Operations::Made(_) => Ok(()),
                    })
            }
            _ => Ok(()),
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
        if state != known.state {
            changed(&planned.name, state);
        }
    }
    left
}
