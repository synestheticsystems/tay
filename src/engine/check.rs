//! Each unit as it is now, from a check made now. A daemon that did not
//! bring the units up finds them again: a unit that is made is checked, and
//! one that runs is found where it runs and looked after again, as the table
//! in `docs/engine.md` says. `tay status` asks it of the units the daemon
//! knows: a unit that is made and was ready is checked again, a group is as
//! its needs are, and a unit that runs is as its task last saw it, which it
//! sees without pause for as long as it runs.

use std::sync::Arc;

use super::fill::{self, HostListen, UnitValues};
use super::settled::{Settled, Standing};
use super::state::{State, UnitState};
use super::supervisor::{Begin, Supervisor};
use super::units::Units;
use crate::disk::Environment;
use crate::module::{self, Found, Made, Operations, Outcome, UnitKind, UnitPlace, Values};
use crate::plan::PlannedUnit;
use crate::value::{Filled, Placeholder, Unfilled};

/// A unit's state as a check found it.
pub struct CheckedState {
    pub unit: String,
    pub state: UnitState,
}

/// Finds each of `units` again, for a daemon that did not bring them up,
/// and records each with `supervisor`. A unit that runs and is under way is
/// looked after again, whether or not the environment is meant to be up;
/// `up` is whether it is, and so whether what is made is checked and
/// restart policies apply.
pub async fn find_again(supervisor: &Arc<Supervisor>, units: &[Arc<PlannedUnit>], up: bool) {
    let mut pass = Pass::default();
    for planned in units {
        let found = match &planned.mechanism {
            Some(mechanism) if matches!(mechanism.kind, UnitKind::Runs(_)) => {
                pass.running(supervisor, planned, up).await
            }
            // What is made is left as it is on disk while the environment
            // is down, and is not asked for.
            _ if !up => continue,
            _ => match pass.blocked(planned) {
                Some(waiting) => Some((waiting, None)),
                None if planned.mechanism.is_none() => Some((
                    UnitState::new(State::Ready, "everything it needs is ready"),
                    None,
                )),
                None => Some(
                    pass.made(planned, &supervisor.environment, &|placeholder, given| {
                        supervisor.fill(placeholder, given)
                    }),
                ),
            },
        };
        // A unit looked after again is recorded by its task.
        let Some((state, values)) = found else {
            pass.settled.record(&planned.name, State::Starting);
            continue;
        };
        pass.record(planned, &state, &values);
        supervisor.record(&planned.name, state, values);
    }
}

/// Each unit the daemon knows, checked now where its state can be: one
/// that is made and was ready is checked again, and a group that was up is
/// as its needs are. Any other unit is as its state was recorded, which for
/// a unit that runs is as its task last saw it.
pub fn check_now(
    units: &Units,
    environment: &Arc<Environment>,
    host_listen: &HostListen,
) -> Vec<CheckedState> {
    let fill = |placeholder: &Placeholder, given: &UnitValues| {
        fill::fill(placeholder, environment, host_listen, given)
    };
    let mut pass = Pass::default();
    units
        .iter()
        .map(|known| {
            let planned = &known.planned;
            let recorded = (known.state.clone(), known.values.clone());
            let kind = planned.mechanism.as_ref().map(|mechanism| &mechanism.kind);
            let checked = matches!(
                (kind, known.state.state),
                (None, State::Ready | State::Waiting) | (Some(UnitKind::Made), State::Ready)
            );
            let (state, values) = match pass.blocked(planned) {
                _ if !checked => recorded,
                Some(waiting) => (waiting, None),
                None if planned.mechanism.is_none() => (
                    UnitState::new(State::Ready, "everything it needs is ready"),
                    None,
                ),
                None => pass.made(planned, environment, &fill),
            };
            pass.record(planned, &state, &values);
            CheckedState {
                unit: planned.name.clone(),
                state,
            }
        })
        .collect()
}

/// How a pass fills a placeholder of a unit, from what the units before it
/// gave.
type Fill<'a> = dyn Fn(&Placeholder, &UnitValues) -> Result<Filled, Unfilled> + 'a;

/// One pass over the units, in their order.
#[derive(Default)]
struct Pass {
    settled: Settled,
    given: UnitValues,
}

impl Pass {
    fn record(&mut self, planned: &PlannedUnit, state: &UnitState, values: &Option<Values>) {
        if state.state != State::Waiting {
            self.settled.record(&planned.name, state.state);
        }
        if let Some(values) = values {
            self.given.insert(planned.name.clone(), values.clone());
        }
    }

    /// The unit's state when a need of it is not ready.
    fn blocked(&mut self, planned: &PlannedUnit) -> Option<UnitState> {
        let standing = self.settled.standing(planned);
        if let Standing::Blocked(failed) = &standing {
            self.settled.blocked(&planned.name, failed.clone());
        }
        standing.waiting()
    }

    /// The unit's operations, its settings filled by `fill` from what the
    /// units before it in this pass gave.
    fn operations(&self, planned: &PlannedUnit, fill: &Fill<'_>) -> Result<Operations, String> {
        let mechanism = planned
            .mechanism
            .as_ref()
            .expect("only a unit with a mechanism is checked");
        mechanism
            .operations(&mut |placeholder| fill(placeholder, &self.given))
            .map_err(|why| why.to_string())
    }

    /// A unit that is made, as a check of it finds it now.
    fn made(
        &self,
        planned: &PlannedUnit,
        environment: &Arc<Environment>,
        fill: &Fill<'_>,
    ) -> (UnitState, Option<Values>) {
        let place = UnitPlace {
            unit: planned.name.clone(),
            environment: Arc::clone(environment),
        };
        match self.operations(planned, fill) {
            Err(reason) => (UnitState::new(State::Failed, reason), None),
            Ok(Operations::Made(made)) => made_now(made.as_ref(), &place),
            Ok(Operations::Running(_)) => unreachable!("a unit whose kind is made is made"),
        }
    }

    /// A unit that runs, found where it runs. One found under way, ended
    /// while no daemon ran, or lost is looked after again, its task saying
    /// how it stands: `None` then. One under way is looked after whatever
    /// its needs are doing; one that is not, and whose need has failed, is
    /// blocked, as it would be were it about to start.
    async fn running(
        &mut self,
        supervisor: &Arc<Supervisor>,
        planned: &Arc<PlannedUnit>,
        up: bool,
    ) -> Option<(UnitState, Option<Values>)> {
        let place = supervisor.place(&planned.name);
        let blocked = match self.settled.standing(planned) {
            Standing::Blocked(failed) => {
                self.settled.blocked(&planned.name, failed.clone());
                Standing::Blocked(failed).waiting()
            }
            Standing::Clear | Standing::Waiting(_) => None,
        };
        let stopped = |detail: String| Some((UnitState::new(State::Stopped, detail), None));
        let not_running = |state: UnitState| match &blocked {
            Some(waiting) => Some((waiting.clone(), None)),
            None => Some((state, None)),
        };
        let fill =
            |placeholder: &Placeholder, given: &UnitValues| supervisor.fill(placeholder, given);
        let running = match self.operations(planned, &fill) {
            Ok(Operations::Running(running)) => running,
            Ok(Operations::Made(_)) => unreachable!("a unit whose kind runs runs"),
            Err(why) => return unfilled(planned, &place, &why, up, blocked).await,
        };
        let begin = match Arc::clone(&running).found(place.clone()).await {
            Err(failure) => return not_running(UnitState::new(State::Failed, failure.to_string())),
            Ok(Found::Never) => {
                return not_running(UnitState::new(
                    State::Stopped,
                    "it has not been started here",
                ));
            }
            Ok(Found::UnderWay(under_way)) => Begin::UnderWay(under_way),
            Ok(Found::Ended {
                outcome: Outcome::Done { values, detail },
                ..
            }) => {
                return Some((UnitState::new(State::Ready, detail), Some(values)));
            }
            Ok(Found::Ended {
                outcome: Outcome::Failed(reason),
                ..
            }) => {
                return match up {
                    true => not_running(UnitState::new(State::Failed, reason)),
                    false => stopped(reason),
                };
            }
            Ok(Found::Ended {
                attempt,
                outcome: outcome @ Outcome::Ended { .. },
            }) if up && blocked.is_none() => Begin::Ended { attempt, outcome },
            Ok(Found::Ended {
                attempt,
                outcome: Outcome::Ended { detail, .. },
            }) => {
                let ended = UnitState::new(State::Stopped, format!("attempt {attempt} {detail}"));
                return match up {
                    true => not_running(ended),
                    false => Some((ended, None)),
                };
            }
            Ok(Found::Lost { attempt }) => Begin::Lost { attempt },
        };
        // Until its task first says how it stands, it is said to be found,
        // so that no one reads the state the table gave it before.
        let found = match &begin {
            Begin::UnderWay(under_way) => {
                Some(format!("attempt {}, found under way", under_way.attempt))
            }
            Begin::Ended { attempt, .. } => Some(format!("attempt {attempt}, found ended")),
            Begin::Lost { attempt } => Some(format!(
                "attempt {attempt}, found with no record of how it ended"
            )),
            Begin::Fresh => None,
        };
        if let Some(found) = found {
            supervisor.record(&planned.name, UnitState::new(State::Starting, found), None);
        }
        supervisor.tend(planned, running, begin, up);
        None
    }
}

/// A unit that runs whose settings cannot be filled, so that it cannot be
/// looked after: if anything of it runs, it is reported so, and `tay down`
/// still stops it from its place alone.
async fn unfilled(
    planned: &PlannedUnit,
    place: &UnitPlace,
    why: &str,
    up: bool,
    blocked: Option<UnitState>,
) -> Option<(UnitState, Option<Values>)> {
    let Some(UnitKind::Runs(runs)) = planned.mechanism.as_ref().map(|mechanism| &mechanism.kind)
    else {
        return None;
    };
    let state = match module::anything_left(runs, place).await {
        Ok(true) => UnitState::new(
            State::Failed,
            format!(
                "something of it is left, but tay cannot fill its settings to look after it: {why}"
            ),
        ),
        Ok(false) if !up => UnitState::new(State::Stopped, "the environment is down"),
        Ok(false) => blocked.unwrap_or_else(|| UnitState::new(State::Failed, why)),
        Err(failure) => UnitState::new(State::Failed, failure.to_string()),
    };
    Some((state, None))
}

/// A unit that is made, as a check of it finds it now.
fn made_now(made: &dyn Made, place: &UnitPlace) -> (UnitState, Option<Values>) {
    match made.check(place) {
        Ok(exists) => (
            UnitState::new(State::Ready, exists.detail),
            Some(exists.values),
        ),
        Err(failure) => (UnitState::new(State::Failed, failure.to_string()), None),
    }
}
