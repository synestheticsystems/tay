//! Each unit as it is now, from a check made now: a unit that is made is
//! checked, one that runs is asked how it stands, and a group is as its
//! needs are. A daemon that did not bring the units up finds them again
//! this way, and `tay status` asks it of the units the daemon knows.

use std::sync::Arc;

use super::fill::{self, UnitValues};
use super::settled::Settled;
use super::settled::Standing;
use super::state::{State, UnitState};
use super::units::Units;
use crate::disk::Environment;
use crate::module::{Made, Operations, UnitKind, UnitPlace, Values};
use crate::plan::PlannedUnit;

/// A unit's state as a check found it, and what it gives if it is made
/// and ready.
pub struct Found {
    pub unit: String,
    pub state: UnitState,
    pub values: Option<Values>,
}

/// Each of `units`, of an environment meant to be up, as it is found now
/// by a daemon that did not bring it up.
pub fn find_again(units: &[Arc<PlannedUnit>], environment: &Environment) -> Vec<Found> {
    let mut pass = Pass::default();
    units
        .iter()
        .map(|planned| {
            let (state, values) = match pass.blocked(planned) {
                Some(waiting) => (waiting, None),
                None => match &planned.mechanism {
                    None => (
                        UnitState::new(State::Ready, "everything it needs is ready"),
                        None,
                    ),
                    Some(_) => pass.found(planned, environment),
                },
            };
            pass.record(planned, &state, &values);
            Found {
                unit: planned.name.clone(),
                state,
                values,
            }
        })
        .collect()
}

/// Each unit the daemon knows, checked now where its state can be: one
/// that is made and was ready is checked again, one that runs and was
/// under way is asked whether it has ended, and a group that was up is as
/// its needs are. Any other unit is as its state was recorded.
pub fn check_now(units: &Units, environment: &Environment) -> Vec<Found> {
    let mut pass = Pass::default();
    units
        .iter()
        .map(|known| {
            let planned = &known.planned;
            let recorded = (known.state.clone(), known.values.clone());
            let kind = planned.mechanism.as_ref().map(|mechanism| &mechanism.kind);
            let checked = matches!(
                (kind, known.state.state),
                (None, State::Ready | State::Waiting)
                    | (Some(UnitKind::Made), State::Ready)
                    | (Some(UnitKind::Runs(_)), State::Ready | State::Starting)
            );
            let (state, values) = match pass.blocked(planned) {
                _ if !checked => recorded,
                Some(waiting) => (waiting, None),
                None if planned.mechanism.is_none() => (
                    UnitState::new(State::Ready, "everything it needs is ready"),
                    None,
                ),
                None => pass.checked(planned, environment).unwrap_or(recorded),
            };
            pass.record(planned, &state, &values);
            Found {
                unit: planned.name.clone(),
                state,
                values,
            }
        })
        .collect()
}

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

    /// A unit that is made, checked; one that runs, found again.
    fn found(
        &self,
        planned: &PlannedUnit,
        environment: &Environment,
    ) -> (UnitState, Option<Values>) {
        let place = UnitPlace {
            unit: &planned.name,
            environment,
        };
        match self.operations(planned, environment) {
            Err(reason) => (UnitState::new(State::Failed, reason), None),
            Ok(Operations::Made(made)) => made_now(made.as_ref(), &place),
            Ok(Operations::Running(running)) => match running.find(&place) {
                Ok(attempt) => match attempt {},
                Err(failure) => (UnitState::new(State::Failed, failure.to_string()), None),
            },
        }
    }

    /// A unit that was ready or under way, as it is now; `None` when it is
    /// as it was recorded.
    fn checked(
        &self,
        planned: &PlannedUnit,
        environment: &Environment,
    ) -> Option<(UnitState, Option<Values>)> {
        let place = UnitPlace {
            unit: &planned.name,
            environment,
        };
        match self.operations(planned, environment) {
            Err(reason) => Some((UnitState::new(State::Failed, reason), None)),
            Ok(Operations::Made(made)) => Some(made_now(made.as_ref(), &place)),
            Ok(Operations::Running(running)) => match running.ended(&place) {
                Ok(None) => None,
                Ok(Some(ending)) => match ending {},
                Err(failure) => Some((UnitState::new(State::Failed, failure.to_string()), None)),
            },
        }
    }

    /// The unit's operations, its settings filled from what the units
    /// before it in this pass gave.
    fn operations(
        &self,
        planned: &PlannedUnit,
        environment: &Environment,
    ) -> Result<Operations, String> {
        let mechanism = planned
            .mechanism
            .as_ref()
            .expect("only a unit with a mechanism is checked");
        mechanism
            .settings
            .fill(&mut |placeholder| fill::fill(placeholder, environment, &self.given))
            .map_err(|why| why.to_string())
    }
}

/// A unit that is made, as a check of it finds it now.
fn made_now(made: &dyn Made, place: &UnitPlace<'_>) -> (UnitState, Option<Values>) {
    match made.check(place) {
        Ok(exists) => (
            UnitState::new(State::Ready, exists.detail),
            Some(exists.values),
        ),
        Err(failure) => (UnitState::new(State::Failed, failure.to_string()), None),
    }
}
