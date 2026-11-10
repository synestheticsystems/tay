//! Looking after one unit that runs, from its first attempt until it is
//! stopped or can run no more: each attempt driven by its module, its
//! readiness checked within its own `within`, counted from when its program
//! started, and again for as long as it runs, so that `unready` and a return
//! to `ready` are both seen; and when an attempt ends, its `restart` decides
//! what follows, as `restarts.rs` says.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::{Supervisor, stop_where_it_runs};
use crate::clock;
use crate::disk::Count;
use crate::disk::unit_log::UnitLog;
use crate::engine::readiness::Probe;
use crate::engine::state::{State, UnitState};
use crate::module::{
    Driving, Outcome, ReadyWithoutCheck, Running, Step, UnderWay, UnitKind, Values,
};
use crate::plan::{PlannedUnit, Readiness};

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

/// A readiness check being made, which gives the check back with what it
/// found.
type CheckUnderWay = Pin<Box<dyn Future<Output = (Probe, Result<(), String>)> + Send>>;

pub(super) struct Tending {
    pub(super) supervisor: Arc<Supervisor>,
    pub(super) planned: Arc<PlannedUnit>,
    pub(super) running: Arc<dyn Running>,
    pub(super) restarts: bool,
    pub(super) stops: mpsc::UnboundedReceiver<Stop>,
    /// Attempts in a row that did not reach ready.
    pub(super) in_a_row: u64,
    /// How the attempt before this one ended, as its details say it.
    pub(super) earlier: Option<String>,
    /// Why the count of attempts could not be kept, said with each state
    /// from then on.
    pub(super) unkept: Option<String>,
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
        unkept: None,
    };
    tending.run(begin).await;
}

impl Tending {
    async fn run(&mut self, begin: Begin) {
        // An attempt found is counted as the daemon that began it counted.
        let found = |attempt| self.supervisor.counts.of(&self.planned.name, attempt);
        let (mut attempt, mut watched) = match begin {
            Begin::Fresh => (1, self.attempt(1, None, false).await),
            Begin::UnderWay(under_way) => {
                let count = found(under_way.attempt).unwrap_or_default();
                self.in_a_row = count.in_a_row;
                let attempt = under_way.attempt;
                (
                    attempt,
                    self.attempt(attempt, Some(under_way), count.ready).await,
                )
            }
            Begin::Ended { attempt, outcome } => {
                let count = found(attempt).unwrap_or_default();
                self.in_a_row = count.in_a_row;
                (
                    attempt,
                    Watched::Came {
                        outcome,
                        ready: count.ready,
                    },
                )
            }
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
            watched = self.attempt(attempt, None, false).await;
        }
    }

    pub(super) fn unit(&self) -> &str {
        &self.planned.name
    }

    pub(super) fn record(&self, state: State, detail: String, values: Option<Values>) {
        self.supervisor
            .record(self.unit(), UnitState::new(state, detail), values);
    }

    /// What a person is told of attempt `attempt` doing `doing`, with how
    /// the one before it ended.
    pub(super) fn detail(&self, attempt: u32, doing: &str) -> String {
        let mut detail = format!("attempt {attempt}, {doing}");
        for more in [&self.earlier, &self.unkept].into_iter().flatten() {
            detail.push_str("; ");
            detail.push_str(more);
        }
        detail
    }

    /// Keeps the count of attempt `attempt`, which is `ready` or not yet.
    fn keep_count(&mut self, attempt: u32, ready: bool) {
        let count = Count {
            attempt,
            in_a_row: self.in_a_row,
            ready,
        };
        self.unkept = self
            .supervisor
            .counts
            .keep(&self.planned.name, count)
            .err()
            .map(|error| format!("its count of attempts cannot be kept: {error}"));
    }

    /// Runs attempt `attempt`, begun now or found `under_way`, and watches
    /// it until it comes out or is stopped. One found under way that `was
    /// ready` is judged as a ready unit is: a check that fails makes it
    /// unready, and it is not stopped for not becoming ready in time.
    async fn attempt(
        &mut self,
        attempt: u32,
        under_way: Option<UnderWay>,
        was_ready: bool,
    ) -> Watched {
        let planned = Arc::clone(&self.planned);
        let mechanism = planned
            .mechanism
            .as_ref()
            .expect("a unit that runs has a mechanism");
        let place = self.supervisor.place(self.unit());
        // The check is filled before the attempt begins, since nothing it
        // uses changes while the attempt runs.
        let check = match &mechanism.ready {
            Readiness::Check { check, within } => {
                let given = self.supervisor.given();
                let filled = check.try_map(&mut |value| {
                    value
                        .fill(&mut |placeholder| self.supervisor.fill(placeholder, &given))
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
        let driving: Driving = match under_way {
            None => {
                self.keep_count(attempt, false);
                self.record(State::Starting, self.detail(attempt, "starting"), None);
                Arc::clone(&self.running).start(place.clone(), attempt, steps)
            }
            Some(under_way) => Arc::clone(&self.running).resume(place.clone(), under_way, steps),
        };
        let mut driving = after_its_steps(driving);
        let mut probe: Option<Probe> = None;
        let mut making: Option<CheckUnderWay> = None;
        let mut next_check: Option<Instant> = None;
        let mut deadline: Option<Instant> = None;
        // Whether the check passes now, whether it ever has, and whether it
        // has been made at all: an attempt found under way past its
        // `within` is judged by one check made now. Why it last did not.
        let (mut passing, mut ready, mut made_once) = (was_ready, was_ready, false);
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
                // A step the attempt came to is taken in before how it came
                // out, which follows it.
                Some(step) = told.recv() => match step {
                    Step::Before(doing) => {
                        self.record(State::Starting, self.detail(attempt, &doing), None);
                    }
                    Step::Running { since } => match (&check, &mechanism.ready) {
                        (Some((filled, within, _)), _) => {
                            let log = UnitLog::at(self.supervisor.environment.layout.unit_log(self.unit()));
                            let made = Probe::of(filled, log, since);
                            let (state, doing) = match ready {
                                true => (State::Ready, format!("ready when it was found again; {} is checked again", made.words())),
                                false => (State::Starting, format!("waiting to be ready: {}", made.words())),
                            };
                            self.record(state, self.detail(attempt, &doing), None);
                            let gone = Duration::from_millis(clock::now_millis().saturating_sub(since));
                            deadline = Some(Instant::now() + within.saturating_sub(gone));
                            probe = Some(made);
                            next_check = Some(Instant::now());
                        }
                        (None, Readiness::WithoutCheck(ReadyWithoutCheck::Started)) => {
                            passing = true;
                            ready = true;
                            self.in_a_row = 0;
                            self.keep_count(attempt, true);
                            self.record(State::Ready, self.detail(attempt, "running"), None);
                        }
                        // A unit its module judges was ready when it was
                        // found again is ready until its module says not.
                        (None, _) => {
                            let (state, doing) = match ready {
                                true => (State::Ready, "running; ready when it was found again"),
                                false => (State::Starting, "running"),
                            };
                            self.record(state, self.detail(attempt, doing), None);
                        }
                    },
                    // The module judges readiness only for a unit that
                    // writes no check; one that writes its own is judged by
                    // that alone.
                    Step::Passes(words) if check.is_none() && !passing => {
                        passing = true;
                        let again = if ready { " again" } else { "" };
                        if !ready {
                            self.keep_count(attempt, true);
                        }
                        ready = true;
                        self.in_a_row = 0;
                        self.record(State::Ready, self.detail(attempt, &format!("{words}{again}")), None);
                    }
                    Step::Fails(why) if check.is_none() => match passing {
                        true => {
                            passing = false;
                            self.record(State::Unready, format!("attempt {attempt}: {why}"), None);
                        }
                        false if !ready => {
                            self.record(State::Starting, self.detail(attempt, &format!("waiting to be ready: {why}")), None);
                        }
                        false => {}
                    },
                    Step::Passes(_) | Step::Fails(_) => {}
                },
                outcome = &mut driving => return Watched::Came { outcome, ready },
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
                            if !ready {
                                self.keep_count(attempt, true);
                            }
                            ready = true;
                            self.in_a_row = 0;
                            self.record(State::Ready, self.detail(attempt, &format!("{words} passes{again}")), None);
                            next_check = (!stays).then(|| Instant::now() + CHECK_READY);
                        }
                        (Ok(()), true) => next_check = (!stays).then(|| Instant::now() + CHECK_READY),
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

    /// Stops what of the unit runs, whatever step it is at, and waits until
    /// nothing of it is left.
    pub(super) async fn stop_now(&self) -> Result<(), String> {
        let place = self.supervisor.place(self.unit());
        let mechanism = self
            .planned
            .mechanism
            .as_ref()
            .expect("a unit that runs has a mechanism");
        match &mechanism.kind {
            UnitKind::Runs(runs) => stop_where_it_runs(runs, &place).await,
            UnitKind::Made => Ok(()),
        }
    }
}

/// `driving`, made to wait one turn once it has come out, so that each step
/// it told of in the same turn as it came out, as one that ends at once
/// does, is taken in before how it came out.
fn after_its_steps(driving: Driving) -> Driving {
    Box::pin(async move {
        let outcome = driving.await;
        tokio::task::yield_now().await;
        outcome
    })
}
