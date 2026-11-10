//! A process as a record holds it: its number, and when it started, which
//! tells it from another process the system gives the same number later, so
//! that tay never signals a process it did not start.

use serde::{Deserialize, Serialize};

/// A process, as a number and when it started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedProcess {
    pub pid: i32,
    /// As [`started`] tells it; `None` when it could not be told, and then
    /// no process is taken to be this one.
    pub started: Option<u64>,
}

impl RecordedProcess {
    /// The process `pid`, which must not have been waited for, so that its
    /// number is its own.
    pub fn of(pid: u32) -> RecordedProcess {
        let pid = i32::try_from(pid).expect("the system's process numbers fit in an i32");
        RecordedProcess {
            pid,
            started: started(pid),
        }
    }
}

/// When the process `pid` started; `None` when there is no such process,
/// or this account may not know.
#[cfg(target_os = "macos")]
pub fn started(pid: i32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: `proc_pidinfo` writes at most `size` bytes to `info`, which is
    // that big, and says how many it wrote.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // SAFETY: the whole structure was written.
    let info = unsafe { info.assume_init() };
    Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

/// When the process `pid` started, in clock ticks since the system booted,
/// as `/proc/<pid>/stat` says; `None` when there is no such process.
#[cfg(not(target_os = "macos"))]
pub fn started(pid: i32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The program's name is in parentheses and may hold anything, spaces
    // and parentheses too; the fields after it are numbers. The start time
    // is the 22nd field, the 20th after the name.
    let (_, after_name) = stat.rsplit_once(')')?;
    after_name.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_that_runs_has_a_start_and_one_that_has_gone_has_none() {
        let mut first = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let mut second = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let pid = |child: &std::process::Child| i32::try_from(child.id()).unwrap();
        let (one, two) = (started(pid(&first)), started(pid(&second)));
        assert!(one.is_some() && two.is_some());
        assert_eq!(started(pid(&first)), one, "it does not change");
        assert_eq!(RecordedProcess::of(first.id()).started, one);
        first.kill().unwrap();
        second.kill().unwrap();
        let gone = pid(&first);
        first.wait().unwrap();
        second.wait().unwrap();
        assert_eq!(started(gone), None);
        assert!(started(std::process::id() as i32).is_some());
    }
}
