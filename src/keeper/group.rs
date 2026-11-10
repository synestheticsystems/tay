//! The process group a unit's program leads. Everything the program starts
//! in turn is in it unless it leaves, so that stopping the group stops what
//! the unit started. A process that puts itself in a session of its own
//! leaves it and escapes the stop; that limit is known and accepted.

use std::time::{Duration, Instant};

use rustix::io::Errno;
use rustix::process::{Pid, Signal, kill_process_group, test_kill_process_group};

use super::record::Process;
use super::started::started;

/// How long a group has to end once it is asked to, before what is left of
/// it is killed.
pub const GRACE: Duration = Duration::from_secs(5);

/// How long the system has to end a group once it is killed.
const KILLED: Duration = Duration::from_secs(2);

/// How often a group that is ending is looked at.
const LOOK: Duration = Duration::from_millis(50);

/// The process group a recorded process leads.
#[derive(Clone, Copy, Debug)]
pub struct Group {
    leader: Process,
}

impl Group {
    pub fn led_by(leader: Process) -> Group {
        Group { leader }
    }

    fn id(&self) -> Option<Pid> {
        Pid::from_raw(self.leader.pid)
    }

    /// Whether anything of the group is left. A group is the one recorded
    /// when its leader is the process recorded, or when its leader has gone
    /// and processes remain in it: the system gives no new process the
    /// number of a group that still has any, so none of them can be
    /// another's.
    pub fn is_left(&self) -> bool {
        let Some(id) = self.id() else {
            return false;
        };
        match (started(self.leader.pid), self.leader.started) {
            (Some(now), Some(recorded)) if now == recorded => true,
            // The leader's number belongs to another process now, so the
            // group the leader led has ended; or which process it is cannot
            // be told, and it is left alone.
            (Some(_), _) => false,
            (None, _) => test_kill_process_group(id).is_ok(),
        }
    }

    /// Sends `signal` to every process of the group, if anything of it is
    /// left.
    pub fn signal(&self, signal: Signal) -> Result<(), Errno> {
        match self.id() {
            Some(id) if self.is_left() => match kill_process_group(id, signal) {
                Ok(()) | Err(Errno::SRCH) => Ok(()),
                Err(errno) => Err(errno),
            },
            _ => Ok(()),
        }
    }

    /// Asks every process of the group to end, waits for as long as
    /// [`GRACE`], and then kills what is left. Whether nothing is left.
    pub fn stop(&self) -> Result<bool, Errno> {
        self.signal(Signal::TERM)?;
        if self.gone_within(GRACE) {
            return Ok(true);
        }
        self.signal(Signal::KILL)?;
        Ok(self.gone_within(KILLED))
    }

    fn gone_within(&self, limit: Duration) -> bool {
        let until = Instant::now() + limit;
        while self.is_left() {
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(LOOK);
        }
        true
    }
}
