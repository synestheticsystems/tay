//! What follows an attempt that has come out: its `restart` decides.
//! `attempts` counts attempts in a row that did not reach ready, and reaching
//! ready starts the count again. A restart waits for everything the unit
//! needs to be ready, and never starts it against a need that has failed.

use std::time::Duration;

use super::tending::Tending;
use crate::diagnostic::quoted_list;
use crate::engine::state::State;
use crate::module::{Outcome, RestartOn};

/// How long a unit waits before it is started again, so that one that ends
/// at once does not start and end for ever without a pause.
const RESTART_PAUSE: Duration = Duration::from_secs(1);

impl Tending {
    /// Records how attempt `attempt` came out, and whether its restart
    /// policy starts it again.
    pub(super) fn restart_after(&mut self, attempt: u32, outcome: Outcome, ready: bool) -> bool {
        let (failed, detail) = match outcome {
            Outcome::Done { values, detail } => {
                let detail = match &self.earlier {
                    Some(earlier) => format!("{detail}; {earlier}"),
                    None => detail,
                };
                self.record(State::Ready, detail, Some(values));
                return false;
            }
            // Of an environment that is down, what does not run is stopped.
            Outcome::Failed(reason) => {
                let state = match self.restarts {
                    true => State::Failed,
                    false => State::Stopped,
                };
                self.record(state, reason, None);
                return false;
            }
            Outcome::Ended { failed, detail } => (failed, detail),
        };
        // Reaching ready starts the count again.
        self.in_a_row = match ready {
            true => 0,
            false => self.in_a_row + 1,
        };
        let ended = format!("attempt {attempt} {detail}");
        if !self.restarts {
            self.record(State::Stopped, ended, None);
            return false;
        }
        let restart = &self
            .planned
            .mechanism
            .as_ref()
            .expect("a unit that runs has a mechanism")
            .restart;
        let why_not = match restart.on {
            RestartOn::Always => None,
            RestartOn::Failure if failed => None,
            on => Some(format!(
                "it is not started again, since its `restart` is `{}`",
                on.word()
            )),
        };
        let used_up = restart
            .attempts
            .filter(|attempts| self.in_a_row >= *attempts);
        match (why_not, used_up) {
            (Some(why_not), _) => {
                self.record(State::Failed, format!("{ended}; {why_not}"), None);
                false
            }
            (None, Some(attempts)) => {
                self.record(
                    State::Failed,
                    format!("{ended}; {attempts} attempts in a row did not become ready"),
                    None,
                );
                false
            }
            (None, None) => {
                self.earlier = Some(ended);
                true
            }
        }
    }

    /// Waits until everything the unit needs is ready and a short pause has
    /// passed; false when it may not start again: a need has failed, or it
    /// was stopped.
    pub(super) async fn ready_to_restart(&mut self, attempt: u32) -> bool {
        let mut changes = self.supervisor.changes();
        loop {
            let mut waiting = Vec::new();
            let mut failed = Vec::new();
            for need in &self.planned.needs {
                match self.supervisor.state(&need.unit).map(|state| state.state) {
                    Some(State::Ready) => {}
                    Some(State::Failed) => failed.push(need.unit.clone()),
                    _ => waiting.push(need.unit.clone()),
                }
            }
            if !failed.is_empty() {
                let failed = quoted_list(&failed);
                self.record(
                    State::Waiting,
                    format!("blocked: {failed} failed before attempt {attempt}"),
                    None,
                );
                return false;
            }
            if waiting.is_empty() {
                break;
            }
            let waiting = quoted_list(&waiting);
            self.record(
                State::Waiting,
                format!("for {waiting}, to start attempt {attempt}"),
                None,
            );
            tokio::select! {
                Some(stop) = self.stops.recv() => {
                    let _ = stop.send(self.stop_now().await);
                    return false;
                }
                changed = changes.changed() => if changed.is_err() {
                    return false;
                },
            }
        }
        tokio::select! {
            Some(stop) = self.stops.recv() => {
                let _ = stop.send(self.stop_now().await);
                false
            }
            () = tokio::time::sleep(RESTART_PAUSE) => true,
        }
    }
}
