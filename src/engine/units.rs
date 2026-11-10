//! What the engine knows of an environment's units: each as it was last
//! planned, its state, and the values it gives once it is made; and each
//! that runs and that the plan does not have, until it is stopped.

use std::sync::Arc;
use std::time::SystemTime;

use super::fill::UnitValues;
use super::state::{State, UnitState};
use crate::module::{Runs, UnitKind, Values};
use crate::plan::PlannedUnit;

/// The units, in an order they could start in, and after them those the
/// plan does not have.
#[derive(Clone, Default)]
pub struct Units {
    known: Vec<Known>,
    unplanned: Vec<Unplanned>,
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

/// A unit that runs, or ran, and that the plan does not have: one the plan
/// left as it changed, or one a daemon found running that its plan does not
/// list. Only where it runs is known of it.
#[derive(Clone)]
pub struct Unplanned {
    pub name: String,
    pub runs: Runs,
    pub state: UnitState,
    pub since: SystemTime,
}

impl Units {
    /// Takes in the units of a plan, in its order: each replaces the unit
    /// of its name, keeping its state, and a unit new to the table is
    /// `stopped`, not having been asked for. A unit that runs and that the
    /// plan does not have is set apart as unplanned, keeping its state; one
    /// that is made, and a group, stay as they are, after them. Gives each
    /// unit of the plan that was set apart, which is to be found again where
    /// it runs.
    pub fn take_in(&mut self, plan: &[Arc<PlannedUnit>]) -> Vec<Arc<PlannedUnit>> {
        let mut before = std::mem::take(&mut self.known);
        let mut returned = Vec::new();
        for planned in plan {
            if self.unplanned(&planned.name).is_some() {
                self.forget(&planned.name);
                returned.push(Arc::clone(planned));
            }
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
        for known in before {
            match known
                .planned
                .mechanism
                .as_ref()
                .map(|mechanism| &mechanism.kind)
            {
                Some(UnitKind::Runs(runs)) => self.unplanned.push(Unplanned {
                    name: known.planned.name.clone(),
                    runs: runs.clone(),
                    state: known.state,
                    since: known.since,
                }),
                Some(UnitKind::Made) | None => self.known.push(known),
            }
        }
        returned
    }

    /// Sets apart `unit`, which runs where `runs` says and which the table
    /// does not have, as unplanned and in `state`; whether it was new.
    pub fn take_in_unplanned(&mut self, unit: &str, runs: Runs, state: UnitState) -> bool {
        if self.get(unit).is_some() || self.unplanned(unit).is_some() {
            return false;
        }
        self.unplanned.push(Unplanned {
            name: unit.to_string(),
            runs,
            state,
            since: SystemTime::now(),
        });
        true
    }

    /// Forgets the unplanned unit `unit`, of which nothing runs any more.
    pub fn forget(&mut self, unit: &str) {
        self.unplanned.retain(|unplanned| unplanned.name != unit);
    }

    /// Records the state `unit` has taken, and what it gives if it gave
    /// anything; whether its state changed. A unit the table does not have
    /// is not recorded.
    pub fn record(&mut self, unit: &str, state: UnitState, values: Option<Values>) -> bool {
        if let Some(known) = self
            .known
            .iter_mut()
            .find(|known| known.planned.name == unit)
        {
            if values.is_some() {
                known.values = values;
            }
            if known.state == state {
                return false;
            }
            known.state = state;
            known.since = SystemTime::now();
            return true;
        }
        let Some(unplanned) = self
            .unplanned
            .iter_mut()
            .find(|unplanned| unplanned.name == unit)
        else {
            return false;
        };
        if unplanned.state == state {
            return false;
        }
        unplanned.state = state;
        unplanned.since = SystemTime::now();
        true
    }

    pub fn get(&self, unit: &str) -> Option<&Known> {
        self.known.iter().find(|known| known.planned.name == unit)
    }

    pub fn unplanned(&self, unit: &str) -> Option<&Unplanned> {
        self.unplanned
            .iter()
            .find(|unplanned| unplanned.name == unit)
    }

    /// The state of `unit`, planned or not.
    pub fn state(&self, unit: &str) -> Option<&UnitState> {
        match self.get(unit) {
            Some(known) => Some(&known.state),
            None => self.unplanned(unit).map(|unplanned| &unplanned.state),
        }
    }

    /// When `unit`, planned or not, took the state it has.
    pub fn since(&self, unit: &str) -> Option<SystemTime> {
        match self.get(unit) {
            Some(known) => Some(known.since),
            None => self.unplanned(unit).map(|unplanned| unplanned.since),
        }
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Known> {
        self.known.iter()
    }

    pub fn each_unplanned(&self) -> impl Iterator<Item = &Unplanned> {
        self.unplanned.iter()
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
