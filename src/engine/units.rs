//! What the engine knows of an environment's units: each of the plan as it
//! was last planned, its state, and the values it gives once it is made;
//! each that runs and that the plan does not have, until it is stopped;
//! and how long each the bring-up under way started has taken.

mod took;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use super::fill::UnitValues;
use super::state::{State, UnitState};
use crate::module::{Runs, UnitKind, Values};
use crate::plan::PlannedUnit;

pub use took::{Timings, Took};

/// The units, in an order they could start in, and after them those the
/// plan does not have.
#[derive(Clone, Default)]
pub struct Units {
    known: Vec<Known>,
    unplanned: Vec<Unplanned>,
    /// How long each unit of the plan that the bring-up under way started
    /// has taken, as the states it takes are recorded.
    pub(super) timings: Timings,
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
    /// How its own program ended, as a shell gives the exit status, once an
    /// attempt of a unit that runs has come out: 0 for one that is done,
    /// `None` for one that did not end by itself.
    pub exit_status: Option<i64>,
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
    /// plan does not have is set apart as unplanned, keeping its state, until
    /// it is stopped; one that is made, and a group, are forgotten, as
    /// nothing of them runs to be stopped. Gives each unit of the plan that
    /// was set apart, which is to be found again where it runs.
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
                    exit_status: None,
                },
            };
            self.known.push(known);
        }
        for known in before {
            let kind = (known.planned.mechanism.as_ref()).map(|mechanism| &mechanism.kind);
            if let Some(UnitKind::Runs(runs)) = kind {
                self.unplanned.push(Unplanned {
                    name: known.planned.name.clone(),
                    runs: runs.clone(),
                    state: known.state,
                    since: known.since,
                });
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
            self.timings.saw(unit, state.state, Instant::now());
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

    /// Records how the attempt of `unit` that came out last ended.
    pub fn exited(&mut self, unit: &str, exit_status: Option<i64>) {
        if let Some(known) = self
            .known
            .iter_mut()
            .find(|known| known.planned.name == unit)
        {
            known.exit_status = exit_status;
        }
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

    /// The units `named` and every unit they need in turn, or wait for
    /// with an `after` that comes up, in the order units start in: what
    /// bringing up `named` brings up.
    pub fn with_needs(&self, named: &[String]) -> Vec<Arc<PlannedUnit>> {
        let mut wanted: HashSet<&str> = named.iter().map(String::as_str).collect();
        // A unit starts after what it needs, so going from the last finds
        // each user before what it needs.
        for known in self.known.iter().rev() {
            let planned = &known.planned;
            if wanted.contains(planned.name.as_str()) {
                let needs = planned.needs.iter().map(|need| need.unit.as_str());
                let after = (planned.after.iter())
                    .filter(|after| after.comes_up)
                    .map(|after| after.unit.as_str());
                wanted.extend(needs.chain(after));
            }
        }
        self.known
            .iter()
            .filter(|known| wanted.contains(known.planned.name.as_str()))
            .map(|known| Arc::clone(&known.planned))
            .collect()
    }

    /// Every unit but `named` that needs one of them or mounts a volume one
    /// of them owns, directly or through other units that do, in the order
    /// units start in: what would hold what `named` keep while it runs.
    pub fn holding(&self, named: &[String]) -> Vec<Arc<PlannedUnit>> {
        let mut held: HashSet<&str> = named.iter().map(String::as_str).collect();
        // Mounting a volume orders nothing, so a unit may start before the
        // one whose volume it mounts: the units are gone over until a pass
        // finds no more.
        let mut found = true;
        while found {
            found = false;
            for known in &self.known {
                let planned = &known.planned;
                if held.contains(planned.name.as_str()) {
                    continue;
                }
                let needs = (planned.needs.iter()).any(|need| held.contains(need.unit.as_str()));
                let mounts = (planned.mechanism.iter())
                    .flat_map(|mechanism| &mechanism.mounts_volumes_of)
                    .any(|owner| held.contains(owner.as_str()));
                if needs || mounts {
                    held.insert(planned.name.as_str());
                    found = true;
                }
            }
        }
        self.known
            .iter()
            .map(|known| &known.planned)
            .filter(|planned| {
                held.contains(planned.name.as_str()) && !named.contains(&planned.name)
            })
            .cloned()
            .collect()
    }

    /// The units that use the values of `unit`, directly or through units
    /// that are made from them, as a process does that reads a port from a
    /// file written with it. A unit that runs is where the chain stops:
    /// what needs it holds nothing of `unit`'s.
    pub fn users(&self, unit: &str) -> Vec<&str> {
        let mut through = vec![unit];
        let mut users: Vec<&str> = Vec::new();
        while let Some(used) = through.pop() {
            for known in &self.known {
                let user = &known.planned;
                if users.contains(&user.name.as_str())
                    || !user.needs.iter().any(|need| need.unit == used)
                {
                    continue;
                }
                users.push(&user.name);
                let made = (user.mechanism.as_ref())
                    .is_some_and(|mechanism| matches!(mechanism.kind, UnitKind::Made));
                if made {
                    through.push(&user.name);
                }
            }
        }
        users
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
