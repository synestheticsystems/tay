//! How a unit stands on what it needs and waits for: everything it needs is
//! ready, some is not ready yet, or some is lost: it has failed, or it was
//! waited for while `unready` longer than its own `within`. A unit whose
//! need is lost stays `waiting` and is blocked: it is never started against
//! a lost need, and its detail names what is lost at once.

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
    /// Waited for while `unready` longer than its own `within`.
    Unready,
    /// Waiting on these units, which are lost.
    Blocked(Lost),
}

/// The units that are lost to whatever needs them: those that failed, and
/// those waited for while `unready` longer than their own `within`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lost {
    pub failed: BTreeSet<String>,
    pub unready: BTreeSet<String>,
}

impl Lost {
    /// Every unit that is lost, by name.
    pub fn names(&self) -> BTreeSet<String> {
        self.failed.union(&self.unready).cloned().collect()
    }

    fn extend(&mut self, other: &Lost) {
        self.failed.extend(other.failed.iter().cloned());
        self.unready.extend(other.unready.iter().cloned());
    }

    fn is_empty(&self) -> bool {
        self.failed.is_empty() && self.unready.is_empty()
    }

    /// What is lost, as a person reads it: "`a` has failed", "`b` is
    /// unready", or both.
    pub fn words(&self) -> String {
        let said = |units: &BTreeSet<String>, one: &str, more: &str, what: &str| {
            let verb = if units.len() == 1 { one } else { more };
            (!units.is_empty()).then(|| format!("{} {verb} {what}", quoted_list(units)))
        };
        let failed = said(&self.failed, "has", "have", "failed");
        let unready = said(&self.unready, "is", "are", "unready");
        failed
            .into_iter()
            .chain(unready)
            .collect::<Vec<_>>()
            .join(", and ")
    }
}

/// How a unit stands on its needs.
pub enum Standing {
    /// Everything it needs is ready, and what it waits for with `after`
    /// has settled.
    Clear,
    /// These are not ready yet.
    Waiting(Vec<String>),
    /// It can never start in this pass, because these are lost.
    Blocked(Lost),
}

impl Settled {
    /// Records the state `unit` took. A unit that is `waiting` and blocked
    /// is recorded with [`Settled::blocked`].
    pub fn record(&mut self, unit: &str, state: State) {
        let settles = match state {
            State::Ready => Settles::Ready,
            State::Failed => Settles::Failed,
            State::Waiting | State::Starting | State::Unready | State::Stopped => {
                self.units.remove(unit);
                return;
            }
        };
        self.units.insert(unit.to_string(), settles);
    }

    pub fn blocked(&mut self, unit: &str, by: Lost) {
        self.units.insert(unit.to_string(), Settles::Blocked(by));
    }

    /// Records that `unit` was waited for while `unready` longer than its
    /// own `within`: it is lost to this pass, as one that failed is.
    pub fn stayed_unready(&mut self, unit: &str) {
        self.units.insert(unit.to_string(), Settles::Unready);
    }

    /// Whether `unit` is lost to this pass because it stayed unready.
    pub fn stayed_unready_is(&self, unit: &str) -> bool {
        matches!(self.units.get(unit), Some(Settles::Unready))
    }

    pub fn is_ready(&self, unit: &str) -> bool {
        matches!(self.units.get(unit), Some(Settles::Ready))
    }

    /// Whether `unit` can no longer become ready in this pass.
    pub fn is_lost(&self, unit: &str) -> bool {
        matches!(
            self.units.get(unit),
            Some(Settles::Failed | Settles::Unready | Settles::Blocked(_))
        )
    }

    /// The units that are lost that `unit` waits on, itself if it is lost.
    pub fn failures_under(&self, unit: &str) -> BTreeSet<String> {
        match self.units.get(unit) {
            Some(Settles::Failed | Settles::Unready) => BTreeSet::from([unit.to_string()]),
            Some(Settles::Blocked(by)) => by.names(),
            Some(Settles::Ready) | None => BTreeSet::new(),
        }
    }

    /// How `planned` stands: on the units it needs, and on the units it
    /// waits for with `after` that come up, which need only have settled.
    pub fn standing(&self, planned: &PlannedUnit) -> Standing {
        let needs = planned.needs.iter().map(|need| need.unit.as_str());
        let mut waiting = match self.standing_on(needs) {
            Standing::Clear => Vec::new(),
            Standing::Waiting(waiting) => waiting,
            blocked @ Standing::Blocked(_) => return blocked,
        };
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

    /// How whatever needs `needs` stands on them.
    pub fn standing_on<'a>(&self, needs: impl IntoIterator<Item = &'a str>) -> Standing {
        let mut lost = Lost::default();
        let mut waiting = Vec::new();
        for need in needs {
            match self.units.get(need) {
                Some(Settles::Ready) => {}
                Some(Settles::Failed) => {
                    lost.failed.insert(need.to_string());
                }
                Some(Settles::Unready) => {
                    lost.unready.insert(need.to_string());
                }
                Some(Settles::Blocked(by)) => lost.extend(by),
                None => waiting.push(need.to_string()),
            }
        }
        match (lost.is_empty(), waiting.is_empty()) {
            (false, _) => Standing::Blocked(lost),
            (true, true) => Standing::Clear,
            (true, false) => Standing::Waiting(waiting),
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
            Standing::Blocked(lost) => Some(UnitState::new(
                State::Waiting,
                format!("blocked: {}", lost.words()),
            )),
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
            asked: crate::disk::Asking::default(),
            home: None,
            shared_folders: None,
            action: None,
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
    fn a_unit_whose_need_stayed_unready_is_blocked_by_it_as_by_one_that_failed() {
        let units = planned(UNITS);
        let mut settled = Settled::default();
        settled.stayed_unready("api");
        settled.record("web-port", State::Failed);
        assert!(settled.is_lost("api"));
        assert_eq!(
            detail(&settled.standing(named(&units, "web"))).as_deref(),
            Some("blocked: `web-port` has failed, and `api` is unready")
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
