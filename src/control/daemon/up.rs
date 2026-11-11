//! Bringing units up: `tay up` as the daemon makes it.

use std::sync::Arc;

use super::change::Change;
use super::{Daemon, log, options, planned};
use crate::control::protocol::{Event, UpRequest};
use crate::disk::{Asked, Intent};
use crate::engine::{self, Decision, Leaving, Outdated, Telling};
use crate::module::Occasion;
use crate::plan::{Options, Plan, PlannedUnit};

impl Daemon {
    /// Brings up what `request` asks for, joined with what the environment
    /// remembers, telling `change`'s followers each change of state and,
    /// once it is decided, the decision.
    pub async fn bring_up(self: Arc<Self>, change: Arc<Change>, request: UpRequest) {
        log(&format!("change: {}", change.describe()));
        // What is remembered is read while this change holds its place, so
        // that no `tay set` lands between reading and writing it. The
        // environment is meant to be up from now, and with what was asked,
        // so that a daemon started after this one dies finds its units
        // again, and every later command sees the same stack.
        let layout = &self.environment.layout;
        let asked = match Asked::read(layout) {
            Ok(remembered) => request.joined(remembered),
            Err(error) => {
                self.fail(&change, &error.to_string());
                return;
            }
        };
        if let Err(error) = Intent::Up.write(layout).and_then(|()| asked.write(layout)) {
            self.fail(&change, &error.to_string());
            return;
        }
        let options = options(asked, request.home);
        if let Some(plan) = self.plan(&change, options).await {
            self.bring_up_plan(&change, plan).await;
        }
    }

    /// Brings `plan` up as `tay up` does: what the plan no longer has
    /// leaves first, its `undo`s run once what they wait for is ready and
    /// what runs of it stopped, and every unit of the plan is brought up,
    /// telling `change`'s followers each change of state and the decision;
    /// then the change is over.
    pub(super) async fn bring_up_plan(&self, change: &Arc<Change>, plan: Plan) {
        let asked = plan.up.clone();
        let production = plan.production;
        let Some(units) = self.take_in_plan(change, plan).await else {
            return;
        };
        if let Err(why) = engine::find_unplanned(&self.supervisor).await {
            self.fail(
                change,
                &format!("what is left of units the plan does not have cannot be told: {why}"),
            );
            return;
        }
        let owed = match engine::undos_owed(&self.supervisor, &units) {
            Ok(owed) => owed,
            Err(why) => {
                let reason = format!(
                    "what the units the plan does not have keep to undo cannot be told: {why}"
                );
                self.fail(change, &reason);
                return;
            }
        };
        let mut said: Vec<String> = units.iter().map(|unit| unit.name.clone()).collect();
        let snapshot = self.supervisor.snapshot();
        let unplanned = snapshot.each_unplanned().map(|unplanned| &unplanned.name);
        for unit in unplanned.chain(owed.iter().map(|owed| &owed.unit)) {
            if !said.contains(unit) {
                said.push(unit.clone());
            }
        }
        change.say(Event::Units { units: said });
        // A production profile refuses a unit with `watch`, and nothing on
        // a server is watched in any case.
        if !production {
            for notice in self.supervisor.watch(&units).await {
                change.say(Event::Notice { notice });
            }
        }
        let leaving = Leaving::new(&self.supervisor, &units, owed);
        let settled = self
            .settle(
                change,
                &units,
                &asked,
                Outdated::Replace,
                Some(leaving),
                &mut |decision| conclude(change, Event::Decided(decision)),
            )
            .await;
        self.end(change, settled);
    }

    /// Plans the stack with `options`, as `tay plan` would; `None` when it
    /// does not plan, which `change`'s followers are told, and the change
    /// is over.
    pub(super) async fn plan(&self, change: &Arc<Change>, options: Options) -> Option<Plan> {
        let root = self.environment.layout.root().to_path_buf();
        match tokio::task::spawn_blocking(move || planned(&root, &options)).await {
            Ok(Ok(plan)) => Some(plan),
            Ok(Err(unplanned)) => {
                change.say(Event::Errors {
                    lines: unplanned.lines,
                    too_old: unplanned.too_old,
                });
                self.finish(change);
                None
            }
            Err(error) => {
                self.fail(
                    change,
                    &format!("tay failed while planning the stack: {error}"),
                );
                None
            }
        }
    }

    /// Takes in the units of `plan`, once what they need of the machine is
    /// there: the stack's shared folder made, and `${host.listen}` learnt.
    /// A unit set apart that the plan has again, as one a daemon found
    /// running that its plan then lacked, is found again where it runs,
    /// and not started a second time. Gives the plan's units; `None` when
    /// the change has failed.
    pub(super) async fn take_in_plan(
        &self,
        change: &Arc<Change>,
        plan: Plan,
    ) -> Option<Vec<Arc<PlannedUnit>>> {
        let shared = self.environment.machine.shared(&plan.stack);
        if let Err(error) = std::fs::create_dir_all(&shared) {
            let reason = format!(
                "the shared folder `{}` cannot be made: {error}",
                shared.display()
            );
            self.fail(change, &reason);
            return None;
        }
        // `${host.listen}` is learnt where units run before any unit's
        // settings are filled.
        if plan.uses_host_listen {
            self.supervisor
                .learn_host_listen(Occasion::BringingUp)
                .await;
        }
        let units: Vec<Arc<PlannedUnit>> = plan.units.into_iter().map(Arc::new).collect();
        let returned = self.supervisor.take_in(&units);
        engine::find_again(&self.supervisor, &returned, true).await;
        Some(units)
    }

    /// Brings up `units`, in their order, until each has settled, doing
    /// with a unit that runs from other settings what `outdated` says, and
    /// first, with `leaving`, what it says, telling `change`'s followers
    /// each change of state; once it is decided whether everything in
    /// `asked` is ready, the decision goes to `decided`. Why the change
    /// came to no decision though nothing cancelled it, when it did not.
    pub(super) async fn settle(
        &self,
        change: &Arc<Change>,
        units: &[Arc<PlannedUnit>],
        asked: &[String],
        outdated: Outdated,
        leaving: Option<Leaving>,
        decided: &mut (dyn FnMut(Decision) + Send),
    ) -> Result<(), String> {
        let mut told = |telling: Telling| match telling {
            Telling::Decided(decision) => {
                log(&format!("{}: {}", change.describe(), words(&decision)));
                decided(decision);
            }
            Telling::Notice(notice) => change.say(Event::Notice { notice }),
        };
        let decided = engine::bring_up(
            &self.supervisor,
            units,
            asked,
            outdated,
            leaving,
            &mut told,
            change.cancelled(),
        )
        .await;
        if decided.is_some() {
            return Ok(());
        }
        match change.cancelled_by() {
            Some(by) => {
                log(&format!("{}: cancelled by {by}", change.describe()));
                change.say(Event::Cancelled { by: by.to_string() });
                Ok(())
            }
            // Every unit settles or is cancelled, so this is a fault in tay,
            // which is said rather than left to hang the change.
            None => Err("tay ended the change before it was decided".to_string()),
        }
    }

    /// The change is over, as [`Daemon::settle`] says it settled.
    pub(super) fn end(&self, change: &Arc<Change>, settled: Result<(), String>) {
        match settled {
            Ok(()) => self.finish(change),
            Err(reason) => self.fail(change, &reason),
        }
    }

    /// A change that could not be made: its followers are told why.
    pub(super) fn fail(&self, change: &Arc<Change>, reason: &str) {
        log(&format!("{}: {reason}", change.describe()));
        change.say(Event::Problem {
            problem: reason.to_string(),
        });
        self.finish(change);
    }
}

/// Tells `change`'s followers `event`, the last thing they are told,
/// though the change may go on.
pub(super) fn conclude(change: &Change, event: Event) {
    change.say(event);
    change.close();
}

/// The decision as the daemon's log says it.
fn words(decision: &Decision) -> String {
    match decision {
        Decision::Ready => "everything asked for is ready".to_string(),
        Decision::Failed { failed } => {
            let names: Vec<&str> = failed.iter().map(|failed| failed.unit.as_str()).collect();
            format!("failed: {}", names.join(", "))
        }
    }
}
