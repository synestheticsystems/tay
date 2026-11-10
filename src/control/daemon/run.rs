//! Running an action: `tay run` as the daemon makes it. The action comes
//! into the plan beside what the environment remembers being asked for,
//! with its arguments, and what it needs is brought up with it as `tay up`
//! would bring them up. Each line it writes is told to whoever follows the
//! change as it is written; once it has come to an end they are told how,
//! and the values it produced, and the action leaves the plan again, so
//! that nothing it gave is kept.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::change::Change;
use super::{Daemon, log, options};
use crate::control::protocol::{Event, Produced, RunRequest};
use crate::disk::unit_log::{Following, UnitLog, Writer};
use crate::disk::{Asked, DiskError};
use crate::engine::Decision;
use crate::plan::PlannedUnit;

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
        // How an earlier run of it ended is not taken for how this one did.
        self.supervisor.exited(&action, None);
        let log = UnitLog::at(self.environment.layout.unit_log(&action));
        let output = Arc::new(Output::from_now(&log, &change));
        let following = tokio::spawn(Arc::clone(&output).follow(Arc::clone(&change)));
        let settled = self
            .settle(
                &change,
                &units,
                std::slice::from_ref(&action),
                &mut |decision| {
                    output.tell(&change);
                    self.ran(&action, decision)
                },
            )
            .await;
        following.abort();
        let after: Vec<Arc<PlannedUnit>> = after.units.into_iter().map(Arc::new).collect();
        self.supervisor.take_in(&after);
        self.end(&change, settled);
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
