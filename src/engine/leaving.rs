//! What `tay up` does first with the units the plan no longer has. A unit
//! that keeps an `undo` to run, as a command done with `when_removed: undo`
//! does, has it run once the units that run that it needed and that the
//! plan has are ready, while those it needed that the plan no longer has
//! are still up, one unit after another, the last kept first; then the unit
//! is forgotten. One whose `undo` fails, or cannot run because a unit it
//! needed has failed or is neither up nor in the plan any more, is said so,
//! and its `undo` is kept and tried again at the next `tay up`. Once every
//! `undo` has come to its end, what runs of the units the plan no longer
//! has is stopped. Until then only the units the `undo`s wait for start,
//! with what those need in turn, so that nothing else the plan brings up
//! meets what the plan no longer has.

use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::settled::{Settled, Standing, have_failed};
use super::state::{State, UnitState};
use super::supervisor::Supervisor;
use super::unplanned::stop_unplanned;
use crate::diagnostic::quoted_list;
use crate::module;
use crate::plan::PlannedUnit;

/// A unit the plan no longer has that keeps an `undo` to run, with the
/// units that run that it needed, or why what it keeps cannot be read.
pub struct Owed {
    pub unit: String,
    needs: Result<Vec<String>, String>,
}

/// The units that keep an `undo` to run and that `plan`, the units of the
/// plan, does not have, the last kept first, each with what it needed.
pub fn undos_owed(supervisor: &Supervisor, plan: &[Arc<PlannedUnit>]) -> Result<Vec<Owed>, String> {
    let kept = module::units_keeping_an_undo(&supervisor.environment)
        .map_err(|failure| failure.to_string())?;
    Ok(kept
        .into_iter()
        .filter(|unit| !plan.iter().any(|planned| planned.name == *unit))
        .map(|unit| {
            let needs = module::kept_undo_needs(&supervisor.place(&unit))
                .map_err(|failure| failure.to_string());
            Owed { unit, needs }
        })
        .collect())
}

/// What a bring-up does first with the units the plan no longer has, and
/// how far it has come.
pub struct Leaving {
    /// The `undo`s owed that have not begun, in the order they run.
    owed: VecDeque<Owed>,
    /// The units of the plan.
    planned: HashSet<String>,
    /// The units of the plan that start before this is over: those the
    /// `undo`s owed wait for, with what they need in turn.
    first: HashSet<String>,
    /// Whether the first `undo` owed has been said to wait.
    waiting_said: bool,
    stage: Stage,
}

enum Stage {
    /// The `undo`s owed run, one at a time.
    Undoing,
    /// What runs of the units the plan no longer has is being stopped.
    Stopping,
    Over,
}

/// What is under way of it: an `undo`, or the stop of what is left.
pub type Step = Pin<Box<dyn Future<Output = ()> + Send>>;

/// How the next `undo` owed stands.
enum Judged {
    /// It runs now.
    Runs,
    /// It waits for these units of the plan.
    Waits(Vec<String>),
    /// It cannot run, as this says.
    Cannot(String),
}

impl Leaving {
    /// What is done first with the units that `plan`, the units of the
    /// plan, no longer has, which owe `owed`.
    pub fn new(supervisor: &Supervisor, plan: &[Arc<PlannedUnit>], owed: Vec<Owed>) -> Leaving {
        let planned: HashSet<String> = plan.iter().map(|planned| planned.name.clone()).collect();
        let waited: Vec<String> = (owed.iter())
            .flat_map(|owed| owed.needs.iter().flatten())
            .filter(|need| planned.contains(*need))
            .cloned()
            .collect();
        let first = (supervisor.snapshot().with_needs(&waited).iter())
            .map(|planned| planned.name.clone())
            .collect();
        Leaving {
            owed: owed.into(),
            planned,
            first,
            waiting_said: false,
            stage: Stage::Undoing,
        }
    }

    /// Whether `unit`, of the plan, waits to start until this is over.
    pub fn holds(&self, unit: &str) -> bool {
        !self.is_over() && !self.first.contains(unit)
    }

    /// Whether every `undo` owed has come to its end and what is left of
    /// the units the plan no longer has has been stopped.
    pub fn is_over(&self) -> bool {
        matches!(self.stage, Stage::Over)
    }

    /// The next step, once the one before it has ended, as the units of the
    /// plan stand in `settled`: the next `undo` owed, once what it waits
    /// for is ready, and once every `undo` has come to its end, the stop of
    /// what is left. Each `undo` that cannot run is said so on the way.
    /// `None` while the next `undo` waits, and once the stop has begun.
    pub fn next(&mut self, supervisor: &Arc<Supervisor>, settled: &Settled) -> Option<Step> {
        if !matches!(self.stage, Stage::Undoing) {
            return None;
        }
        while let Some(owed) = self.owed.pop_front() {
            match self.judged(supervisor, settled, &owed) {
                Judged::Runs => {
                    self.waiting_said = false;
                    return Some(Box::pin(undo(Arc::clone(supervisor), owed.unit)));
                }
                Judged::Waits(waiting) => {
                    if !self.waiting_said {
                        let waits = format!(
                            "it is not in the plan any more, and its `undo` waits for {}",
                            quoted_list(waiting)
                        );
                        supervisor.say(&owed.unit, UnitState::new(State::Waiting, waits));
                        self.waiting_said = true;
                    }
                    self.owed.push_front(owed);
                    return None;
                }
                Judged::Cannot(why) => {
                    self.waiting_said = false;
                    not_undone(supervisor, &owed.unit, &why);
                }
            }
        }
        self.stage = Stage::Stopping;
        let supervisor = Arc::clone(supervisor);
        Some(Box::pin(async move { stop_unplanned(&supervisor).await }))
    }

    /// The step under way has ended.
    pub fn ended(&mut self) {
        if matches!(self.stage, Stage::Stopping) {
            self.stage = Stage::Over;
        }
    }

    /// How `owed` stands, as the units of the plan stand in `settled` and
    /// the units the plan no longer has stand with `supervisor`.
    fn judged(&self, supervisor: &Supervisor, settled: &Settled, owed: &Owed) -> Judged {
        let needs = match &owed.needs {
            Ok(needs) => needs,
            Err(why) => return Judged::Cannot(why.clone()),
        };
        let (in_plan, elsewhere): (Vec<&String>, Vec<&String>) =
            needs.iter().partition(|need| self.planned.contains(*need));
        match settled.standing_on(in_plan.iter().map(|need| need.as_str())) {
            Standing::Clear => {}
            Standing::Waiting(waiting) => return Judged::Waits(waiting),
            Standing::Blocked(failed) => {
                return Judged::Cannot(format!("its `undo` cannot run: {}", have_failed(&failed)));
            }
        }
        let down = elsewhere
            .into_iter()
            .find(|need| (supervisor.state(need)).is_none_or(|state| state.state != State::Ready));
        match down {
            None => Judged::Runs,
            Some(down) => Judged::Cannot(format!(
                "its `undo` cannot run: it needed `{down}`, which is neither up nor in the plan any more"
            )),
        }
    }
}

/// Runs the `undo` `unit` keeps, saying what came of it; once it has run,
/// the unit is forgotten. What of the unit still runs, as of a command that
/// was running when it left the plan, is stopped first, so that nothing of
/// it goes on after its work is undone.
async fn undo(supervisor: Arc<Supervisor>, unit: String) {
    let running = (supervisor.snapshot().unplanned(&unit)).map(|unplanned| unplanned.runs.clone());
    if let Some(runs) = running
        && let Err(why) = supervisor.stop(&unit, &runs).await
    {
        let why = format!("it could not be stopped for its `undo` to run: {why}");
        return not_undone(&supervisor, &unit, &why);
    }
    let undoing = "it is not in the plan any more, so its `undo` runs";
    supervisor.say(&unit, UnitState::new(State::Starting, undoing));
    match module::run_kept_undo(&supervisor.place(&unit)).await {
        Ok(()) => {
            let undone = "it is not in the plan any more, and its `undo` has run";
            supervisor.say(&unit, UnitState::new(State::Stopped, undone));
        }
        Err(why) => not_undone(&supervisor, &unit, &why.to_string()),
    }
}

/// Says that the `undo` of `unit` did not run, as `why` says, and is kept.
fn not_undone(supervisor: &Supervisor, unit: &str, why: &str) {
    let detail =
        format!("it is not in the plan any more, and {why}; the next `tay up` tries it again");
    supervisor.say(unit, UnitState::new(State::Failed, detail));
}
