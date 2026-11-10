//! Bringing units up: `tay up` as the daemon makes it.

use std::sync::Arc;

use super::change::Change;
use super::{Daemon, log, planned};
use crate::control::protocol::{Event, UpRequest};
use crate::disk::Intent;
use crate::engine::{self, Decision};
use crate::module::Occasion;
use crate::plan::PlannedUnit;

impl Daemon {
    /// Brings up what `request` asks for, telling `change`'s followers each
    /// change of state and, once it is decided, the decision.
    pub async fn bring_up(self: Arc<Self>, change: Arc<Change>, request: UpRequest) {
        log(&format!("change: {}", change.describe()));
        // The environment is meant to be up from now, and with what was
        // asked, so that a daemon started after this one dies finds its
        // units again, and every later command sees the same stack.
        let layout = &self.environment.layout;
        if let Err(error) = Intent::Up
            .write(layout)
            .and_then(|()| request.asked.write(layout))
        {
            self.fail(&change, &error.to_string());
            return;
        }
        let root = self.environment.layout.root().to_path_buf();
        let plan = match tokio::task::spawn_blocking(move || planned(&root, &request)).await {
            Ok(Ok(plan)) => plan,
            Ok(Err(unplanned)) => {
                change.say(Event::Errors {
                    lines: unplanned.lines,
                    too_old: unplanned.too_old,
                });
                self.finish(&change);
                return;
            }
            Err(error) => {
                self.fail(
                    &change,
                    &format!("tay failed while planning the stack: {error}"),
                );
                return;
            }
        };
        let shared = self.environment.machine.shared(&plan.stack);
        if let Err(error) = std::fs::create_dir_all(&shared) {
            let reason = format!(
                "the shared folder `{}` cannot be made: {error}",
                shared.display()
            );
            self.fail(&change, &reason);
            return;
        }
        // `${host.listen}` is learnt where units run before any unit's
        // settings are filled.
        if plan.uses_host_listen {
            self.supervisor
                .learn_host_listen(Occasion::BringingUp)
                .await;
        }
        let units: Vec<Arc<PlannedUnit>> = plan.units.into_iter().map(Arc::new).collect();
        // What runs of a unit set apart that the plan has again, as one a
        // daemon found running that its plan then lacked, is found again
        // where it runs, and not started a second time.
        let returned = self.supervisor.take_in(&units);
        engine::find_again(&self.supervisor, &returned, true).await;
        if let Err(why) = engine::find_unplanned(&self.supervisor).await {
            self.fail(
                &change,
                &format!("what is left of units the plan does not have cannot be told: {why}"),
            );
            return;
        }
        let unplanned: Vec<String> = self
            .supervisor
            .snapshot()
            .each_unplanned()
            .map(|unplanned| unplanned.name.clone())
            .collect();
        change.say(Event::Units {
            units: units
                .iter()
                .map(|unit| unit.name.clone())
                .chain(unplanned)
                .collect(),
        });
        // What runs and the plan no longer has is stopped first, so that
        // nothing it holds is in the way of what the plan brings up.
        engine::stop_unplanned(&self.supervisor).await;
        for unit in &units {
            if unit
                .mechanism
                .as_ref()
                .is_some_and(|mechanism| !mechanism.watch.is_empty())
            {
                change.say(Event::Notice {
                    notice: format!(
                        "`{}` watches files, but watching is not built yet: it will not be run again when they change",
                        unit.name
                    ),
                });
            }
        }
        self.settle(&change, &units, &plan.up).await;
    }

    /// Brings up `units`, in their order, until each has settled, telling
    /// `change`'s followers each change of state and, once it is decided,
    /// whether everything in `asked` is ready; then the change is over.
    pub(super) async fn settle(
        &self,
        change: &Arc<Change>,
        units: &[Arc<PlannedUnit>],
        asked: &[String],
    ) {
        let mut decided = |decision: Decision| {
            log(&format!("{}: {}", change.describe(), words(&decision)));
            change.say(Event::Decided(decision));
            change.close();
        };
        let mut noticed = |notice: String| change.say(Event::Notice { notice });
        let decided = engine::bring_up(
            &self.supervisor,
            units,
            asked,
            &mut decided,
            &mut noticed,
            change.cancelled(),
        )
        .await;
        if decided.is_none() {
            match change.cancelled_by() {
                Some(by) => {
                    log(&format!("{}: cancelled by {by}", change.describe()));
                    change.say(Event::Cancelled { by: by.to_string() });
                }
                // Every unit settles or is cancelled, so this is a fault in
                // tay, which is said rather than left to hang the change.
                None => {
                    self.fail(change, "tay ended the change before it was decided");
                    return;
                }
            }
        }
        self.finish(change);
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
