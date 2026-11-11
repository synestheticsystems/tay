//! Running an action: `tay run` as the daemon makes it, on an environment
//! that is up or down, which it leaves as it found it. The action comes
//! into the plan beside what the environment remembers being asked for,
//! with its arguments, and what it needs is brought up with it as `tay up`
//! would bring them up, but that a unit that is up is joined as it runs:
//! a run that could only be made by replacing one, to open a port of it the
//! action uses from the host, is refused. Each line the action writes is
//! told to whoever follows the change as it is written. Once it has come to
//! an end, what was brought up for it is stopped again, and they are told
//! how it ended and the values it produced; the action leaves the plan
//! again, so that nothing it gave is kept.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::change::Change;
use super::up::conclude;
use super::{Daemon, log, options};
use crate::control::protocol::{Event, Produced, Refused, RunRequest};
use crate::disk::unit_log::{Following, UnitLog, Writer};
use crate::disk::{Asked, DiskError};
use crate::engine::{Decision, Outdated, State, UnitState, runs};
use crate::plan::{Plan, PlannedUnit};

/// How often the action's log is read for what it has written.
const FOLLOW: Duration = Duration::from_millis(250);

impl Daemon {
    /// Runs the action `request` asks for, after bringing up what it needs,
    /// telling `change`'s followers each change of state, each line it
    /// writes, and what it came to.
    pub async fn run_action(self: Arc<Self>, change: Arc<Change>, request: RunRequest) {
        log(&format!("change: {}", change.describe()));
        let remembered = match Asked::read(&self.environment.layout) {
            Ok(remembered) => remembered,
            Err(error) => {
                self.fail(&change, &error.to_string());
                return;
            }
        };
        let action = request.run.action.clone();
        // The plan the environment holds once the action has run, which is
        // the plan without it.
        let after = options(remembered.clone(), request.home.clone());
        let Some(after) = self.plan(&change, after).await else {
            return;
        };
        let mut with_action = options(remembered, request.home);
        with_action.action = Some(request.run);
        let Some(with_action) = self.plan(&change, with_action).await else {
            return;
        };
        if let Some(refused) = self.would_replace(&action, &with_action, &after) {
            log(&format!("{}: refused: {refused}", change.describe()));
            change.say(Event::Refused(refused));
            self.finish(&change);
            return;
        }
        // An environment never brought up keeps nothing yet, and what the
        // action needs keeps its records there.
        if let Err(error) = self.environment.layout.make_state() {
            self.fail(&change, &error.to_string());
            return;
        }
        if self.take_in_plan(&change, with_action).await.is_none() {
            return;
        }
        let units = self
            .supervisor
            .snapshot()
            .with_needs(std::slice::from_ref(&action));
        change.say(Event::Units {
            units: units.iter().map(|unit| unit.name.clone()).collect(),
        });
        // What runs of them now is left running once the action has ended;
        // the rest is stopped again.
        let brought: Vec<&Arc<PlannedUnit>> = (units.iter())
            .filter(|planned| runs(planned).is_some() && !self.supervisor.is_tended(&planned.name))
            .collect();
        // How an earlier run of it ended is not taken for how this one did.
        self.supervisor.exited(&action, None);
        let log = UnitLog::at(self.environment.layout.unit_log(&action));
        let output = Arc::new(Output::from_now(&log, &change));
        let following = tokio::spawn(Arc::clone(&output).follow(Arc::clone(&change)));
        let mut came_to = None;
        let settled = self
            .settle(
                &change,
                &units,
                std::slice::from_ref(&action),
                Outdated::Join,
                None,
                &mut |decision| came_to = Some(self.ran(&action, decision)),
            )
            .await;
        following.abort();
        output.tell(&change);
        // A run that `tay down` cancelled leaves the stopping to it.
        if change.cancelled_by().is_none() {
            self.stop_again(&change, &brought).await;
        }
        if let Some(ran) = came_to {
            conclude(&change, ran);
        }
        let after: Vec<Arc<PlannedUnit>> = after.units.into_iter().map(Arc::new).collect();
        self.supervisor.take_in(&after);
        self.end(&change, settled);
    }

    /// Why running `action` with the plan `with_action` is refused, when it
    /// is: it opens a published port of a unit that is up, which the plan
    /// without it, `after`, does not open, and the unit could open it only
    /// by being replaced.
    fn would_replace(&self, action: &str, with_action: &Plan, after: &Plan) -> Option<Refused> {
        let (unit, port) = with_action
            .opening_more_than(after)
            .into_iter()
            .find(|(unit, _)| self.supervisor.is_tended(unit))?;
        Some(Refused::WouldReplace {
            problem: format!(
                "running `{action}` would replace `{unit}`, which is up, to open its port {port} on the host; `tay run` never stops, restarts or replaces a unit that is up"
            ),
        })
    }

    /// Stops again each of `brought`, which were brought up for an action
    /// that has ended, the last to start the first to stop, telling
    /// `change`'s followers of each that could not be.
    async fn stop_again(&self, change: &Change, brought: &[&Arc<PlannedUnit>]) {
        for planned in brought.iter().rev() {
            if !self.supervisor.is_tended(&planned.name) {
                continue;
            }
            let state = match self.supervisor.stop_to_replace(planned).await {
                Ok(()) => UnitState::new(
                    State::Stopped,
                    "stopped by `tay run`, which brought it up for the action",
                ),
                Err(why) => {
                    let notice = format!(
                        "`{}` was brought up for the action and could not be stopped again: {why}",
                        planned.name
                    );
                    change.say(Event::Notice {
                        notice: notice.clone(),
                    });
                    UnitState::new(State::Failed, notice)
                }
            };
            self.supervisor.record(&planned.name, state, None);
        }
    }

    /// What running `action` came to, as `decision` says: how its own
    /// program exited, and, once it is ready, the values it produced, each
    /// declared secret without its value.
    fn ran(&self, action: &str, decision: Decision) -> Event {
        let units = self.supervisor.snapshot();
        let known = units.get(action);
        let values = match (&decision, known.and_then(|known| known.values.as_ref())) {
            (Decision::Ready, Some(values)) => values
                .iter()
                .map(|(name, value)| Produced {
                    name: name.clone(),
                    value: value.plain().map(|exact| exact.as_text()),
                })
                .collect(),
            _ => Vec::new(),
        };
        Event::Ran {
            decision,
            exit_status: known.and_then(|known| known.exit_status),
            values,
        }
    }
}

/// What the action writes, read from its log as it is written, from when
/// the run began; nothing more once the log cannot be read, which is said.
struct Output {
    following: Mutex<Option<Following>>,
}

impl Output {
    fn from_now(log: &UnitLog, change: &Change) -> Output {
        let following = log
            .follow_from_now()
            .map_err(|error| unreadable(change, &error))
            .ok();
        Output {
            following: Mutex::new(following),
        }
    }

    /// Tells `change`'s followers what it writes, for as long as it runs.
    async fn follow(self: Arc<Self>, change: Arc<Change>) {
        loop {
            tokio::time::sleep(FOLLOW).await;
            self.tell(&change);
        }
    }

    /// Tells `change`'s followers each line written since the last was
    /// read; tay's own notes among them are not the action's.
    fn tell(&self, change: &Change) {
        let mut following = self
            .following
            .lock()
            .expect("what follows the action's log is not poisoned");
        let Some(log) = following.as_mut() else {
            return;
        };
        let mut lines = Vec::new();
        let read = log.read("", &mut lines);
        for line in lines {
            if line.writer == Writer::Unit {
                change.say(Event::Output { line: line.text });
            }
        }
        if let Err(error) = read {
            unreadable(change, &error);
            *following = None;
        }
    }
}

/// Tells `change`'s followers that what the action writes cannot be read.
fn unreadable(change: &Change, error: &DiskError) {
    change.say(Event::Notice {
        notice: format!("what the action writes cannot be read: {error}"),
    });
}
