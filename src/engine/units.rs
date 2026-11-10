//! What the engine knows of an environment's units: each as it was last
//! planned, its state, and the values it gives once it is made.

use std::sync::Arc;
use std::time::SystemTime;

use super::fill::UnitValues;
use super::state::{State, UnitState};
use crate::module::Values;
use crate::plan::PlannedUnit;

/// The units, in an order they could start in.
#[derive(Clone, Default)]
pub struct Units {
    known: Vec<Known>,
}

/// One unit.
#[derive(Clone)]
pub struct Known {
    pub planned: Arc<PlannedUnit>,
    pub state: UnitState,
    /// When it took its state.
    pub since: SystemTime,
    /// What it gives, once it is made.
    pub values: Option<Values>,
}

impl Units {
    /// Takes in the units of a plan, in its order: each replaces the unit
    /// of its name, keeping its state, and a unit new to the table is
    /// `stopped`, not having been asked for. The units the plan does not
    /// have stay as they are, after them.
    pub fn take_in(&mut self, plan: &[Arc<PlannedUnit>]) {
        let mut before = std::mem::take(&mut self.known);
        for planned in plan {
            let known = match before
                .iter()
                .position(|known| known.planned.name == planned.name)
            {
                Some(index) => Known {
                    planned: Arc::clone(planned),
                    ..before.remove(index)
                },
                None => Known {
                    planned: Arc::clone(planned),
                    state: UnitState::new(State::Stopped, "not asked for"),
                    since: SystemTime::now(),
                    values: None,
                },
            };
            self.known.push(known);
        }
        self.known.append(&mut before);
    }

    /// Records the state `unit` has taken, and what it gives if it gave
    /// anything. A unit the table does not have is not recorded.
    pub fn record(&mut self, unit: &str, state: UnitState, values: Option<Values>) {
        let Some(known) = self
            .known
            .iter_mut()
            .find(|known| known.planned.name == unit)
        else {
            return;
        };
        if values.is_some() {
            known.values = values;
        }
        if known.state != state {
            known.state = state;
            known.since = SystemTime::now();
        }
    }

    pub fn get(&self, unit: &str) -> Option<&Known> {
        self.known.iter().find(|known| known.planned.name == unit)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Known> {
        self.known.iter()
    }

    /// The values each unit that is made and ready gives.
    pub fn given(&self) -> UnitValues {
        self.known
            .iter()
            .filter(|known| known.state.state == State::Ready)
            .filter_map(|known| Some((known.planned.name.clone(), known.values.clone()?)))
            .collect()
    }
}
