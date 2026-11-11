//! How long each unit a bring-up started took, so that a slow bring-up
//! shows where its time went: from the start of its first attempt until it
//! was ready or had failed. The time it spent waiting for what it needs is
//! not its own and is not counted. The times are taken as its states are
//! recorded, on a clock that only goes forward.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::clock;
use crate::engine::state::State;

/// The units the bring-up under way started, each with the time it has
/// taken so far, in the order they started in.
#[derive(Clone, Default)]
pub struct Timings {
    started: Vec<(String, Timing)>,
}

/// The time one unit has taken.
#[derive(Clone)]
struct Timing {
    /// What it took before the stretch being counted.
    counted: Duration,
    /// When the stretch being counted began; `None` while it waits.
    since: Option<Instant>,
    /// The state it took last. Once it is `ready` or `failed`, nothing
    /// more is counted.
    state: State,
}

/// How long one unit took, in seconds, and the state it came to then:
/// `ready` or `failed`, or for one still under way when the bring-up was
/// decided, the state it had.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Took {
    pub unit: String,
    pub seconds: f64,
    pub state: State,
}

impl Timings {
    /// Forgets the units timed so far, for a bring-up that begins: a unit
    /// it does not start is not timed.
    pub fn begin(&mut self) {
        self.started.clear();
    }

    /// Times `unit` from `at`, when the bring-up under way starts it, unless
    /// it was started once already: its time runs from its first attempt.
    pub fn start(&mut self, unit: &str, at: Instant) {
        if self.started.iter().any(|(timed, _)| timed == unit) {
            return;
        }
        let timing = Timing {
            counted: Duration::ZERO,
            since: Some(at),
            state: State::Starting,
        };
        self.started.push((unit.to_string(), timing));
    }

    /// Takes in that `unit` took `state` at `at`.
    pub fn saw(&mut self, unit: &str, state: State, at: Instant) {
        let Some((_, timing)) = self.started.iter_mut().find(|(timed, _)| timed == unit) else {
            return;
        };
        if matches!(timing.state, State::Ready | State::Failed) {
            return;
        }
        timing.state = state;
        match state {
            State::Waiting | State::Ready | State::Failed => {
                if let Some(since) = timing.since.take() {
                    timing.counted += at.saturating_duration_since(since);
                }
            }
            State::Starting | State::Unready | State::Stopped => {
                timing.since.get_or_insert(at);
            }
        }
    }

    /// How long each unit started took, counted up to `at` for one still
    /// under way, the longest first.
    pub fn took(&self, at: Instant) -> Vec<Took> {
        let mut took: Vec<Took> = (self.started.iter())
            .map(|(unit, timing)| {
                let stretch = (timing.since)
                    .map_or(Duration::ZERO, |since| at.saturating_duration_since(since));
                Took {
                    unit: unit.clone(),
                    seconds: clock::in_seconds(timing.counted + stretch),
                    state: timing.state,
                }
            })
            .collect();
        // Units that took as long stay in the order they started in.
        took.sort_by(|one, other| other.seconds.total_cmp(&one.seconds));
        took
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn after(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    fn seconds(took: &[Took]) -> Vec<(&str, f64, State)> {
        (took.iter())
            .map(|took| (took.unit.as_str(), took.seconds, took.state))
            .collect()
    }

    #[test]
    fn a_unit_takes_the_time_it_spent_starting_and_not_what_it_waited() {
        let zero = Instant::now();
        let mut timings = Timings::default();
        timings.start("api", zero);
        timings.saw("api", State::Starting, after(zero, 100));
        // An attempt that ended waits for what it needs before the next.
        timings.saw("api", State::Waiting, after(zero, 1500));
        timings.saw("api", State::Starting, after(zero, 9500));
        timings.saw("api", State::Ready, after(zero, 10_000));
        // Once it is ready nothing more is counted.
        timings.saw("api", State::Starting, after(zero, 11_000));
        timings.saw("api", State::Ready, after(zero, 12_000));
        assert_eq!(
            seconds(&timings.took(after(zero, 20_000))),
            [("api", 2.0, State::Ready)]
        );
    }

    #[test]
    fn units_come_longest_first_and_one_under_way_is_counted_until_now() {
        let zero = Instant::now();
        let mut timings = Timings::default();
        timings.start("quick", zero);
        timings.start("slow", zero);
        timings.start("lasting", after(zero, 250));
        // A unit started twice in one bring-up is timed from the first.
        timings.start("quick", after(zero, 1000));
        timings.saw("quick", State::Ready, after(zero, 1200));
        timings.saw("slow", State::Failed, after(zero, 3000));
        // One no bring-up started is not timed.
        timings.saw("joined", State::Ready, after(zero, 3000));
        assert_eq!(
            seconds(&timings.took(after(zero, 4250))),
            [
                ("lasting", 4.0, State::Starting),
                ("slow", 3.0, State::Failed),
                ("quick", 1.2, State::Ready),
            ]
        );
        timings.begin();
        assert!(timings.took(after(zero, 5000)).is_empty());
    }
}
