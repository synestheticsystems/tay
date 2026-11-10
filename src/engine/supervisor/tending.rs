//! Looking after one unit that runs, from its first attempt until it is
//! stopped or can run no more: each attempt driven by its module, its
//! readiness checked within its own `within`, counted from when its program
//! started, and again for as long as it runs, so that `unready` and a return
//! to `ready` are both seen; and when an attempt ends, its `restart` decides.
//! `attempts` counts attempts in a row that did not reach ready; reaching
//! ready starts the count again. A restart waits for everything the unit
//! needs to be ready, and never starts it against a need that has failed.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::{Supervisor, stop_blocking};
use crate::clock;
use crate::diagnostic::quoted_list;
use crate::engine::fill;
use crate::engine::readiness::Probe;
use crate::engine::state::{State, UnitState};
use crate::module::{
    Driving, Outcome, ReadyWithoutCheck, RestartOn, Running, Step, UnderWay, UnitKind, Values,
};
use crate::plan::{PlannedUnit, Readiness};
use crate::unit_log::UnitLog;

/// How long a unit waits before it is started again, so that one that ends
/// at once does not start and end for ever without a pause.
const RESTART_PAUSE: Duration = Duration::from_secs(1);

/// How often a unit's check is made until it passes: about twice a second,
/// as the document says.
const CHECK_STARTING: Duration = Duration::from_millis(500);

/// How often a ready unit's check is made again.
const CHECK_READY: Duration = Duration::from_secs(5);

/// Where looking after a unit begins.
pub enum Begin {
    /// Its first attempt, which prepares.
    Fresh,
    /// An attempt found under way.
    UnderWay(UnderWay),
    /// Its last attempt, found ended while no daemon ran: its restart
    /// policy decides what follows.
    Ended { attempt: u32, outcome: Outcome },
    /// Its last attempt, found with no record of how it ended: what was left
    /// of it is stopped, and it is reported as exactly that.
    Lost { attempt: u32 },
}

/// A request to stop, answered once nothing of the unit is left.
pub type Stop = oneshot::Sender<Result<(), String>>;

/// How the watch of one attempt ended.
enum Watched {
    /// The attempt came out so; `ready` is whether it was ever ready.
    Came { outcome: Outcome, ready: bool },
    /// It was stopped, as asked.
    Stopped,
}

/// A check being made, which gives the check back with what it found.
type Making = Pin<Box<dyn Future<Output = (Probe, Result<(), String>)> + Send>>;

struct Tending {
    supervisor: Arc<Supervisor>,
    planned: Arc<PlannedUnit>,
    running: Arc<dyn Running>,
    restarts: bool,
    stops: mpsc::UnboundedReceiver<Stop>,
    /// Attempts in a row that did not reach ready.
    in_a_row: u64,
    /// How the attempt before this one ended, as its details say it.
    earlier: Option<String>,
}

/// Looks after `planned` with `running` from `begin`, telling `supervisor`
/// each change of state, until it is stopped through `stops` or can run no
/// more. `restarts` is whether its restart policy applies.
pub async fn tend(
    supervisor: Arc<Supervisor>,
    planned: Arc<PlannedUnit>,
    running: Arc<dyn Running>,
    begin: Begin,
    restarts: bool,
    stops: mpsc::UnboundedReceiver<Stop>,
) {
    let mut tending = Tending {
        supervisor,
        planned,
        running,
        restarts,
        stops,
        in_a_row: 0,
        earlier: None,
    };
    tending.run(begin).await;
}

impl Tending {
    async fn run(&mut self, begin: Begin) {
        let (mut attempt, mut watched) = match begin {
            Begin::Fresh => (1, self.attempt(1, None).await),
            Begin::UnderWay(under_way) => (
                under_way.attempt,
                self.attempt(under_way.attempt, Some(under_way)).await,
            ),
            Begin::Ended { attempt, outcome } => (
                attempt,
                Watched::Came {
                    outcome,
                    ready: false,
                },
            ),
            Begin::Lost { attempt } => {
                let outcome = match self.stop_now().await {
                    Ok(()) => Outcome::Failed(format!(
                        "attempt {attempt} is gone, with no record of how it ended: its keeper was lost, and what was left of it was stopped"
                    )),
                    Err(why) => Outcome::Failed(format!(
                        "attempt {attempt} is gone, with no record of how it ended, and what is left of it cannot be stopped: {why}"
                    )),
                };
                (
                    attempt,
                    Watched::Came {
                        outcome,
                        ready: false,
                    },
                )
            }
        };
        loop {
            let Watched::Came { outcome, ready } = watched else {
                return;
            };
            if !self.restart_after(attempt, outcome, ready)
                || !self.ready_to_restart(attempt + 1).await
            {
                return;
            }
            attempt += 1;
            watched = self.attempt(attempt, None).await;
        }
    }

    fn unit(&self) -> &str {
        &self.planned.name
    }

    fn record(&self, state: State, detail: String, values: Option<Values>) {
        self.supervisor
            .record(self.unit(), UnitState::new(state, detail), values);
    }

    /// What a person is told of attempt `attempt` doing `doing`, with how
    /// the one before it ended.
    fn detail(&self, attempt: u32, doing: &str) -> String {
        match &self.earlier {
            Some(earlier) => format!("attempt {attempt}, {doing}; {earlier}"),
            None => format!("attempt {attempt}, {doing}"),
        }
    }

    /// Runs attempt `attempt`, begun now or found `under_way`, and watches
    /// it until it comes out or is stopped.
    async fn attempt(&mut self, attempt: u32, under_way: Option<UnderWay>) -> Watched {
        let mechanism = self
            .planned
            .mechanism
            .as_ref()
            .expect("a unit that runs has a mechanism");
        let place = self.supervisor.place(self.unit());
        // The check is filled before the attempt begins, since nothing it
        // uses changes while the attempt runs.
        let check = match &mechanism.ready {
            Readiness::Check { check, within } => {
                let given = self.supervisor.given();
                let environment = &self.supervisor.environment;
                let filled = check.try_map(&mut |value| {
                    value
                        .fill(&mut |placeholder| fill::fill(placeholder, environment, &given))
                        .map_err(|why| why.to_string())
                });
                let impossible = filled.as_ref().ok().and_then(Probe::impossible);
                match (filled, impossible) {
                    (_, Some(why)) => {
                        return Watched::Came {
                            outcome: Outcome::Failed(why),
                            ready: false,
                        };
                    }
                    (Ok(filled), None) => Some((filled, within.length(), *within)),
                    (Err(why), None) => {
                        return Watched::Came {
                            outcome: Outcome::Failed(format!(
                                "its readiness check cannot be filled: {why}"
                            )),
                            ready: false,
                        };
                    }
                }
            }
            Readiness::WithoutCheck(_) => None,
        };
        let (steps, mut told) = mpsc::unbounded_channel();
        let mut driving: Driving = match under_way {
            None => {
                self.record(State::Starting, self.detail(attempt, "starting"), None);
                let prepares = attempt == 1;
                Arc::clone(&self.running).start(place.clone(), attempt, prepares, steps)
            }
            Some(under_way) => {
                // The step it was found at is the first it is told.
                let _ = steps.send(under_way.step.clone());
                Arc::clone(&self.running).resume(place.clone(), under_way, steps)
            }
        };
        let mut probe: Option<Probe> = None;
        let mut making: Option<Making> = None;
        let mut next_check: Option<Instant> = None;
        let mut deadline: Option<Instant> = None;
        // Whether the check passes now, whether it ever has, and whether it
        // has been made at all: an attempt found under way past its
        // `within` is judged by one check made now. Why it last did not.
        let (mut passing, mut ready, mut made_once) = (false, false, false);
        let mut last_why = String::new();
        let never = Instant::now() + Duration::from_secs(365 * 24 * 3600);
        loop {
            tokio::select! {
                biased;
                Some(stop) = self.stops.recv() => {
                    drop(driving);
                    let _ = stop.send(self.stop_now().await);
                    return Watched::Stopped;
                }
                outcome = &mut driving => return Watched::Came { outcome, ready },
                Some(step) = told.recv() => match step {
                    Step::Before(doing) => {
                        self.record(State::Starting, self.detail(attempt, &doing), None);
                    }
                    Step::Running { since } => match (&check, &mechanism.ready) {
                        (Some((filled, within, _)), _) => {
                            let log = UnitLog::at(self.supervisor.environment.layout.unit_log(self.unit()));
                            let made = Probe::of(filled, log, since);
                            let doing = format!("waiting to be ready: {}", made.words());
                            self.record(State::Starting, self.detail(attempt, &doing), None);
                            let gone = Duration::from_millis(clock::now_millis().saturating_sub(since));
                            deadline = Some(Instant::now() + within.saturating_sub(gone));
                            probe = Some(made);
                            next_check = Some(Instant::now());
                        }
                        (None, Readiness::WithoutCheck(ReadyWithoutCheck::Started)) => {
                            passing = true;
                            ready = true;
                            self.in_a_row = 0;
                            self.record(State::Ready, self.detail(attempt, "running"), None);
                        }
                        (None, _) => {
                            self.record(State::Starting, self.detail(attempt, "running"), None);
                        }
                    },
                },
                (made, found) = async { making.as_mut().expect("a check is being made").await }, if making.is_some() => {
                    making = None;
                    made_once = true;
                    let words = made.words();
                    let stays = made.stays_passed();
                    probe = Some(made);
                    match (found, passing) {
                        (Ok(()), false) => {
                            passing = true;
                            let again = if ready { " again" } else { "" };
                            ready = true;
                            self.in_a_row = 0;
                            self.record(State::Ready, self.detail(attempt, &format!("{words} passes{again}")), None);
                            next_check = (!stays).then(|| Instant::now() + CHECK_READY);
                        }
                        (Ok(()), true) => next_check = Some(Instant::now() + CHECK_READY),
                        (Err(why), false) => {
                            last_why = why;
                            next_check = Some(Instant::now() + if ready { CHECK_READY } else { CHECK_STARTING });
                        }
                        (Err(why), true) => {
                            passing = false;
                            let detail = format!("attempt {attempt}: {words} no longer passes: {why}");
                            self.record(State::Unready, detail, None);
                            next_check = Some(Instant::now() + CHECK_READY);
                        }
                    }
                }
                () = tokio::time::sleep_until(next_check.unwrap_or(never)), if next_check.is_some() && making.is_none() => {
                    next_check = None;
                    if let (Some(mut made), Some((_, within, _))) = (probe.take(), &check) {
                        // A check is given what is left of `within` until
                        // the unit is first ready, and `within` itself after,
                        // and when an attempt found under way is past its
                        // `within`, for the one check that judges it.
                        let left = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
                        let limit = match left {
                            Some(left) if !ready && !left.is_zero() => left,
                            _ => *within,
                        };
                        let running = Arc::clone(&self.running);
                        let place = place.clone();
                        making = Some(Box::pin(async move {
                            let found = made.once(&running, &place, limit).await;
                            (made, found)
                        }));
                    }
                }
                () = tokio::time::sleep_until(deadline.unwrap_or(never)), if deadline.is_some() && !ready && made_once && making.is_none() => {
                    drop(driving);
                    let (_, _, written) = check.as_ref().expect("a deadline is set by a check");
                    let outcome = match self.stop_now().await {
                        Ok(()) => Outcome::Ended {
                            failed: true,
                            detail: format!("did not become ready within {written}, and was stopped: {last_why}"),
                        },
                        Err(why) => Outcome::Failed(format!(
                            "attempt {attempt} did not become ready within {written}, and cannot be stopped: {why}"
                        )),
                    };
                    return Watched::Came { outcome, ready: false };
                }
            }
        }
    }

    /// Records how attempt `attempt` came out, and whether its restart
    /// policy starts it again.
    fn restart_after(&mut self, attempt: u32, outcome: Outcome, ready: bool) -> bool {
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
        if !ready {
            self.in_a_row += 1;
        }
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
    async fn ready_to_restart(&mut self, attempt: u32) -> bool {
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

    /// Stops what of the unit runs, whatever step it is at, and waits until
    /// nothing of it is left.
    async fn stop_now(&self) -> Result<(), String> {
        let place = self.supervisor.place(self.unit());
        let mechanism = self
            .planned
            .mechanism
            .as_ref()
            .expect("a unit that runs has a mechanism");
        match &mechanism.kind {
            UnitKind::Runs(runs) => stop_blocking(runs.clone(), place).await,
            UnitKind::Made => Ok(()),
        }
    }
}
