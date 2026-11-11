//! What `tay up` does first with the units the plan no longer has. A unit
//! that keeps an `undo` to run, as a command done with `when_removed: undo`
//! does, has it run once the units that run that it needed and that the
//! plan has are ready, while those it needed that the plan no longer has
//! and that are up are still up, one unit after another, the last kept
//! first; then the unit is forgotten. A unit it needed that the plan no
//! longer has and that is not up is not waited for, as nothing would bring
//! it up: the `undo` runs without it, and says so. One whose `undo` fails,
//! or cannot run because a unit it needed that the plan has has failed, is
//! said so, with why noted for `tay status`, and its `undo` is kept and
//! tried again at the next `tay up`, which fails. Once every `undo` has
//! come to its end, what runs of the units the plan no longer has is
//! stopped. Until then only the units the `undo`s wait for start, with
//! what those need in turn, so that nothing else the plan brings up meets
//! what the plan no longer has.

use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::check::CheckedState;
use super::settled::{Settled, Standing, have_failed};
use super::state::{State, UnitState};
use super::supervisor::Supervisor;
use super::unplanned::stop_unplanned;
use crate::diagnostic::quoted_list;
use crate::disk::Environment;
use crate::module::{self, UnitPlace};
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
    let kept = units_keeping_an_undo(&supervisor.environment)?;
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

/// Each unit of `environment` that keeps an `undo` and that `listed` does
/// not have, as `tay status` reports it while the `undo` waits: `failed`,
/// with why it last did not run, or `stopped` until a `tay up` first runs
/// it. `listed` has the units of the plan, whose `undo` waits for nothing.
pub fn undos_waiting(
    environment: &Arc<Environment>,
    listed: impl Fn(&str) -> bool,
) -> Result<Vec<CheckedState>, String> {
    let kept = units_keeping_an_undo(environment)?;
    Ok(kept
        .into_iter()
        .filter(|unit| !listed(unit))
        .map(|unit| {
            let place = UnitPlace {
                unit: unit.clone(),
                environment: Arc::clone(environment),
            };
            let state = match module::undo_waiting(&place) {
                Ok(Some(why)) => UnitState::new(State::Failed, why),
                Ok(None) => UnitState::new(
                    State::Stopped,
                    "it is not in the plan any more: the next `tay up` runs its `undo`",
                ),
                Err(failure) => UnitState::new(
                    State::Failed,
                    format!("it is not in the plan any more, and why its `undo` waits cannot be read: {failure}"),
                ),
            };
            CheckedState { unit, state }
        })
        .collect())
}

/// The units of `environment` that keep an `undo` to run, the last kept
/// first; why they cannot be told, when they cannot.
fn units_keeping_an_undo(environment: &Environment) -> Result<Vec<String>, String> {
    module::units_keeping_an_undo(environment).map_err(|failure| {
        format!("what the units the plan does not have keep to undo cannot be told: {failure}")
    })
}

/// An `undo` owed that did not run: its unit, what it was said with, and
/// whether it ran, so that the last lines it wrote tell why.
pub struct NotUndone {
    pub unit: String,
    pub reason: String,
    pub ran: bool,
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
    /// Each `undo` owed that did not run.
    not_undone: Vec<NotUndone>,
}

enum Stage {
    /// The `undo`s owed run, one at a time.
    Undoing,
    /// What runs of the units the plan no longer has is being stopped.
    Stopping,
    Over,
}

/// What is under way of it: an `undo`, which gives how it did not run when
/// it did not, or the stop of what is left.
pub type Step = Pin<Box<dyn Future<Output = Option<NotUndone>> + Send>>;

/// How the next `undo` owed stands.
enum Judged {
    /// It runs now, without these units it needed, which the plan no
    /// longer has and which are not up.
    Runs(Vec<String>),
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
            not_undone: Vec::new(),
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

    /// Each `undo` owed that has come to its end without running.
    pub fn not_undone(&self) -> &[NotUndone] {
        &self.not_undone
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
                Judged::Runs(without) => {
                    self.waiting_said = false;
                    let undo = undo(Arc::clone(supervisor), owed.unit, without);
                    return Some(Box::pin(undo));
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
                    let not_undone = not_undone(supervisor, &owed.unit, &why, false);
                    self.not_undone.push(not_undone);
                }
            }
        }
        self.stage = Stage::Stopping;
        let supervisor = Arc::clone(supervisor);
        Some(Box::pin(async move {
            stop_unplanned(&supervisor).await;
            None
        }))
    }

    /// The step under way has ended, as `came` says of an `undo` that did
    /// not run.
    pub fn ended(&mut self, came: Option<NotUndone>) {
        self.not_undone.extend(came);
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
        let without = elsewhere
            .into_iter()
            .filter(|need| (supervisor.state(need)).is_none_or(|state| state.state != State::Ready))
            .cloned()
            .collect();
        Judged::Runs(without)
    }
}

/// What is said of an `undo` that runs without `without`, the units it
/// needed that are neither up nor in the plan any more; nothing when it has
/// them all.
fn said_without(without: &[String]) -> String {
    match without {
        [] => String::new(),
        [_] => format!(
            " without {}, which it needed and which is neither up nor in the plan any more",
            quoted_list(without)
        ),
        _ => format!(
            " without {}, which it needed and which are neither up nor in the plan any more",
            quoted_list(without)
        ),
    }
}

/// Runs the `undo` `unit` keeps, saying what came of it, and that it runs
/// without `without`, the units it needed that are neither up nor in the
/// plan any more; once it has run, the unit is forgotten. What of the unit
/// still runs, as of a command that was running when it left the plan, is
/// stopped first, so that nothing of it goes on after its work is undone.
/// How it did not run, when it did not.
async fn undo(
    supervisor: Arc<Supervisor>,
    unit: String,
    without: Vec<String>,
) -> Option<NotUndone> {
    let running = (supervisor.snapshot().unplanned(&unit)).map(|unplanned| unplanned.runs.clone());
    if let Some(runs) = running
        && let Err(why) = supervisor.stop(&unit, &runs).await
    {
        let why = format!("it could not be stopped for its `undo` to run: {why}");
        return Some(not_undone(&supervisor, &unit, &why, false));
    }
    let without = said_without(&without);
    let undoing = format!("it is not in the plan any more, so its `undo` runs{without}");
    supervisor.say(&unit, UnitState::new(State::Starting, undoing));
    match module::run_kept_undo(&supervisor.place(&unit)).await {
        Ok(()) => {
            let undone = "it is not in the plan any more, and its `undo` has run";
            supervisor.say(&unit, UnitState::new(State::Stopped, undone));
            None
        }
        Err(why) if without.is_empty() => {
            Some(not_undone(&supervisor, &unit, &why.to_string(), true))
        }
        Err(why) => {
            let why = format!("{why}, having run{without}");
            Some(not_undone(&supervisor, &unit, &why, true))
        }
    }
}

/// Says that the `undo` of `unit` did not run, as `why` says, and is kept,
/// noting why for `tay status`; `ran` is whether it ran.
fn not_undone(supervisor: &Supervisor, unit: &str, why: &str, ran: bool) -> NotUndone {
    let mut reason =
        format!("it is not in the plan any more, and {why}; the next `tay up` tries it again");
    if let Err(failure) = module::note_undo_waiting(&supervisor.place(unit), &reason) {
        reason.push_str(&format!(
            "; why it waits could not be kept for `tay status`: {failure}"
        ));
    }
    supervisor.say(unit, UnitState::new(State::Failed, reason.clone()));
    NotUndone {
        unit: unit.to_string(),
        reason,
        ran,
    }
}
