//! The one change a daemon makes at a time: what it is, since when, and
//! everything it has said. Each command that follows a change reads what it
//! said from the start, so a second `tay up` that joins the first gets the
//! same result, how long it took included.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use http_body_util::channel::Sender;
use hyper::body::{Bytes, Frame};
use tokio::sync::watch;

use crate::clock;
use crate::control::protocol::{Event, Refused, Restart, RunRequest, UpRequest, WipeRequest};

/// What a change does. It has no `Debug`, since a `--set` it carries may
/// hold a secret setting's value.
#[derive(Clone, PartialEq, Eq)]
pub enum Kind {
    Up(UpRequest),
    Down,
    Restart(Restart),
    Run(RunRequest),
    Wipe(WipeRequest),
}

/// Why a change is not begun, though no other change is under way.
pub enum NotBegun {
    /// What was asked does not fit the stack, as said.
    DoesNotFit(String),
    /// It is refused as things stand: what was asked needs something that
    /// is not up.
    Refused(Refused),
}

pub struct Change {
    pub kind: Kind,
    /// When it began, in seconds since 1970.
    pub since: u64,
    /// When it began, on a clock that only goes forward, for saying how
    /// long it took.
    began: Instant,
    said: Mutex<Said>,
    /// How many events have been said, for followers to wait on.
    told: watch::Sender<usize>,
    cancelled: watch::Sender<bool>,
    /// What cancelled it, once something has: the command or event, as a
    /// person reads it.
    cancelled_by: OnceLock<&'static str>,
    ended: watch::Sender<bool>,
}

#[derive(Default)]
struct Said {
    events: Vec<Event>,
    /// Whether everything a follower is told has been said.
    closed: bool,
}

impl Change {
    pub fn new(kind: Kind) -> Arc<Change> {
        Arc::new(Change {
            kind,
            since: clock::now(),
            began: Instant::now(),
            said: Mutex::default(),
            told: watch::Sender::new(0),
            cancelled: watch::Sender::new(false),
            cancelled_by: OnceLock::new(),
            ended: watch::Sender::new(false),
        })
    }

    /// The change as the command that asked for it is written.
    pub fn describe(&self) -> String {
        match &self.kind {
            Kind::Up(request) => std::iter::once("tay up")
                .chain(request.adds.units.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
            Kind::Down => "tay down".to_string(),
            Kind::Restart(restart) => std::iter::once("tay restart")
                .chain(restart.units.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
            Kind::Wipe(WipeRequest::Units { units, .. }) => std::iter::once("tay wipe")
                .chain(units.iter().map(String::as_str))
                .chain(std::iter::once("--yes"))
                .collect::<Vec<_>>()
                .join(" "),
            Kind::Wipe(WipeRequest::All) => "tay wipe --all --yes".to_string(),
            Kind::Run(request) => std::iter::once("tay run")
                .chain(std::iter::once(request.run.action.as_str()))
                .chain(request.run.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    /// How long the change has taken so far, in seconds.
    pub fn seconds_so_far(&self) -> f64 {
        clock::in_seconds(self.began.elapsed())
    }

    /// Tells every follower `event`. Once the change is closed, followers
    /// have been told its end, and nothing more is said to them.
    pub fn say(&self, event: Event) {
        let mut said = self
            .said
            .lock()
            .expect("a change's events are not poisoned");
        if said.closed {
            return;
        }
        said.events.push(event);
        let count = said.events.len();
        drop(said);
        self.told.send_replace(count);
    }

    /// Ends what followers are told: they end once they have read all
    /// that has been said. The change itself may go on.
    pub fn close(&self) {
        self.said
            .lock()
            .expect("a change's events are not poisoned")
            .closed = true;
        self.told.send_modify(|_| {});
    }

    /// Stops the change, `by` the command or event that stops it. What
    /// first stops it is what it says stopped it.
    pub fn cancel(&self, by: &'static str) {
        // A change stopped twice was stopped by the first.
        let _ = self.cancelled_by.set(by);
        self.cancelled.send_replace(true);
    }

    /// Becomes true when the change is to stop.
    pub fn cancelled(&self) -> watch::Receiver<bool> {
        self.cancelled.subscribe()
    }

    /// What stopped the change, once something has.
    pub fn cancelled_by(&self) -> Option<&'static str> {
        self.cancelled_by.get().copied()
    }

    /// Marks the change as over: nothing it began is under way.
    pub fn end(&self) {
        self.ended.send_replace(true);
    }

    /// Waits until the change is over.
    pub async fn ended(&self) {
        let mut ended = self.ended.subscribe();
        // The sender is the change's own, so it outlives this wait.
        let _ = ended.wait_for(|ended| *ended).await;
    }

    /// Sends what the change has said and says, one JSON line each, until
    /// it is closed or the follower has gone.
    pub async fn follow(&self, mut to: Sender<Bytes>) {
        let mut told = self.told.subscribe();
        let mut next = 0;
        loop {
            told.borrow_and_update();
            let (events, closed) = {
                let said = self
                    .said
                    .lock()
                    .expect("a change's events are not poisoned");
                (said.events[next..].to_vec(), said.closed)
            };
            next += events.len();
            for event in &events {
                let mut line = serde_json::to_vec(event).expect("an event serializes");
                line.push(b'\n');
                if to.send(Frame::data(Bytes::from(line))).await.is_err() {
                    return;
                }
            }
            if closed || told.changed().await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;
    use http_body_util::channel::Channel;

    use super::*;
    use crate::engine::State;

    fn ready(unit: &str) -> Event {
        Event::State {
            unit: unit.to_string(),
            state: State::Ready,
            detail: String::new(),
            at: 0,
        }
    }

    #[tokio::test]
    async fn a_follower_reads_from_the_start_up_to_the_end_and_nothing_after_it() {
        let change = Change::new(Kind::Down);
        change.say(ready("first"));
        let (to, body) = Channel::new(4);
        let follower = Arc::clone(&change);
        let following = tokio::spawn(async move { follower.follow(to).await });
        change.say(Event::Down { left: Vec::new() });
        change.close();
        // The change goes on after its end, as `tay up` does once it is
        // decided; followers have been told all they are told.
        change.say(ready("after"));
        following.await.unwrap();
        let read = body.collect().await.unwrap().to_bytes();
        let events: Vec<Event> = read
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], Event::State { unit, .. } if unit == "first"));
        assert!(events[1].ends_change());
    }

    #[test]
    fn what_first_cancels_a_change_is_what_cancelled_it() {
        let change = Change::new(Kind::Down);
        assert_eq!(change.cancelled_by(), None);
        change.cancel("`tay down`");
        change.cancel("the daemon's exit");
        assert_eq!(change.cancelled_by(), Some("`tay down`"));
        assert!(*change.cancelled().borrow());
    }
}
