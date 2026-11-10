//! Bringing units up: `tay up` as the daemon makes it.

use std::sync::Arc;

use super::change::Change;
use super::{Daemon, log, planned};
use crate::control::protocol::{Event, UpRequest};
use crate::disk::Intent;
use crate::engine::{self, Decision, Progress};
use crate::plan::PlannedUnit;

impl Daemon {
    /// Brings up what `asked` asks for, telling `change`'s followers each
    /// change of state and, once it is decided, the decision.
    pub async fn bring_up(self: Arc<Self>, change: Arc<Change>, asked: UpRequest) {
        log(&format!("change: {}", change.describe()));
        // The environment is meant to be up from now, so that a daemon
        // started after this one dies finds its units again.
        if let Err(error) = Intent::Up.write(&self.environment.layout) {
            self.fail(&change, &error.to_string());
            return;
        }
        let root = self.environment.layout.root().to_path_buf();
        let plan = match tokio::task::spawn_blocking(move || planned(&root, &asked)).await {
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
        let units: Vec<Arc<PlannedUnit>> = plan.units.into_iter().map(Arc::new).collect();
        self.units
            .lock()
            .expect("the units are not poisoned")
            .take_in(&units);
        change.say(Event::Units {
            units: units.iter().map(|unit| unit.name.clone()).collect(),
        });
        let mut progress = |progress: Progress| match progress {
            Progress::Changed {
                unit,
                state,
                values,
            } => self.changed(&change, &unit, state, values),
            Progress::Decided(decision) => {
                log(&format!("{}: {}", change.describe(), words(&decision)));
                change.say(Event::Up(decision));
                change.close();
            }
        };
        let decided = engine::bring_up(
            &units,
            &plan.up,
            &self.environment,
            &mut progress,
            change.cancelled(),
        )
        .await;
        if decided.is_none() {
            let by = change
                .cancelled_by()
                .expect("a change ends undecided only when it is cancelled");
            log(&format!("{}: cancelled by {by}", change.describe()));
            change.say(Event::Cancelled { by: by.to_string() });
        }
        self.finish(&change);
    }

    /// A change that could not be made: its followers are told why.
    fn fail(&self, change: &Arc<Change>, reason: &str) {
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
