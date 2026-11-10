//! How a unit stands on what it needs and waits for: everything it needs is
//! ready, some is not ready yet, or some has failed. A unit whose need has
//! failed stays `waiting` and is blocked: it is never started against a
//! failed need, and its detail names what failed at once.

use std::collections::{BTreeSet, HashMap};

use super::state::{State, UnitState};
use crate::diagnostic::quoted_list;
use crate::plan::PlannedUnit;

/// What the units seen so far in one pass over a plan settled as.
#[derive(Default)]
pub struct Settled {
    units: HashMap<String, Settles>,
}

enum Settles {
    Ready,
    Failed,
    /// Waiting on these units, which failed.
    Blocked(BTreeSet<String>),
}

/// How a unit stands on its needs.
pub enum Standing {
    /// Everything it needs is ready, and what it waits for with `after`
    /// has settled.
    Clear,
    /// These are not ready yet.
    Waiting(Vec<String>),
    /// It can never start, because these failed.
    Blocked(BTreeSet<String>),
}

impl Settled {
    /// Records the state `unit` took. A unit that is `waiting` and blocked
    /// is recorded with [`Settled::blocked`].
    pub fn record(&mut self, unit: &str, state: State) {
        let settles = match state {
            State::Ready => Settles::Ready,
            State::Failed => Settles::Failed,
            State::Waiting | State::Starting | State::Stopped => {
                self.units.remove(unit);
                return;
            }
        };
        self.units.insert(unit.to_string(), settles);
    }

    pub fn blocked(&mut self, unit: &str, by: BTreeSet<String>) {
        self.units.insert(unit.to_string(), Settles::Blocked(by));
    }

    pub fn is_ready(&self, unit: &str) -> bool {
        matches!(self.units.get(unit), Some(Settles::Ready))
    }

    /// Whether `unit` can no longer become ready in this pass.
    pub fn is_lost(&self, unit: &str) -> bool {
        matches!(
            self.units.get(unit),
            Some(Settles::Failed | Settles::Blocked(_))
        )
    }

    /// The units that failed that `unit` waits on, itself if it failed.
    pub fn failures_under(&self, unit: &str) -> BTreeSet<String> {
        match self.units.get(unit) {
            Some(Settles::Failed) => BTreeSet::from([unit.to_string()]),
            Some(Settles::Blocked(by)) => by.clone(),
            Some(Settles::Ready) | None => BTreeSet::new(),
        }
    }

    /// How `planned` stands: on the units it needs, and on the units it
    /// waits for with `after` that come up, which need only have settled.
    pub fn standing(&self, planned: &PlannedUnit) -> Standing {
        let mut failed = BTreeSet::new();
        let mut waiting = Vec::new();
        for need in &planned.needs {
            match self.units.get(&need.unit) {
                Some(Settles::Ready) => {}
                Some(Settles::Failed) => {
                    failed.insert(need.unit.clone());
                }
                Some(Settles::Blocked(by)) => failed.extend(by.iter().cloned()),
                None => waiting.push(need.unit.clone()),
            }
        }
        if !failed.is_empty() {
            return Standing::Blocked(failed);
        }
        for after in planned.after.iter().filter(|after| after.comes_up) {
            if !self.units.contains_key(&after.unit) {
                waiting.push(after.unit.clone());
            }
        }
        match waiting.is_empty() {
            true => Standing::Clear,
            false => Standing::Waiting(waiting),
        }
    }
}

impl Standing {
    /// The state of a unit that stands so and is not under way: `None`
    /// when it is clear to start.
    pub fn waiting(&self) -> Option<UnitState> {
        match self {
            Standing::Clear => None,
            Standing::Waiting(units) => Some(UnitState::new(
                State::Waiting,
                format!("for {}", quoted_list(units)),
            )),
            Standing::Blocked(failed) => {
                let failed: Vec<&String> = failed.iter().collect();
                let verb = if failed.len() == 1 { "has" } else { "have" };
                Some(UnitState::new(
                    State::Waiting,
                    format!("blocked: {} {verb} failed", quoted_list(failed)),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Diagnostics;
    use crate::diagnostic::source::Sources;
    use crate::plan::{self, Options, Outcome};

    /// The units of a stack written out here, as the planner plans them.
    fn planned(units: &str) -> Vec<PlannedUnit> {
        let root = tempfile::tempdir().unwrap();
        let stack =
            format!("format: 1\nstack: settling\nup: [web, notice, search]\nunits:\n{units}");
        std::fs::write(root.path().join("tay.yaml"), stack).unwrap();
        let options = Options {
            profile: None,
            sets: Vec::new(),
            units: Vec::new(),
            home: None,
            shared_folders: None,
        };
        let (mut sources, mut d) = (Sources::default(), Diagnostics::default());
        match plan::plan(root.path(), &options, &mut sources, &mut d) {
            Outcome::Plan(plan) => plan.units,
            _ => panic!("the stack plans: {:?}", d.into_sorted()),
        }
    }

    fn named<'p>(units: &'p [PlannedUnit], name: &str) -> &'p PlannedUnit {
        units.iter().find(|unit| unit.name == name).unwrap()
    }

    fn detail(standing: &Standing) -> Option<String> {
        standing.waiting().map(|state| state.detail)
    }

    const UNITS: &str = "  web: { needs: [api, web-port] }\n  api: { needs: [database] }\n  database: { needs: [] }\n  web-port: { needs: [] }\n  notice: { uses: port, after: [search] }\n  search: { needs: [] }\n";

    #[test]
    fn a_unit_waits_for_what_it_needs_and_is_blocked_by_what_failed_through_others() {
        let units = planned(UNITS);
        let mut settled = Settled::default();
        let web = named(&units, "web");
        assert_eq!(
            detail(&settled.standing(web)).as_deref(),
            Some("for `api`, `web-port`")
        );
        settled.record("web-port", State::Ready);
        settled.record("database", State::Failed);
        let Standing::Blocked(failed) = settled.standing(named(&units, "api")) else {
            panic!("`api` needs `database`, which failed");
        };
        settled.blocked("api", failed);
        // `web` waits on `api`, which waits on `database`: what failed is
        // named, not the unit in between.
        assert_eq!(
            detail(&settled.standing(web)).as_deref(),
            Some("blocked: `database` has failed")
        );
        assert!(settled.is_lost("api"));
        assert_eq!(
            settled.failures_under("api"),
            BTreeSet::from(["database".to_string()])
        );
    }

    #[test]
    fn a_unit_waits_for_what_it_comes_after_until_that_is_ready_or_failed() {
        let units = planned(UNITS);
        let mut settled = Settled::default();
        let notice = named(&units, "notice");
        assert_eq!(
            detail(&settled.standing(notice)).as_deref(),
            Some("for `search`")
        );
        settled.record("search", State::Failed);
        assert!(matches!(settled.standing(notice), Standing::Clear));
    }
}
