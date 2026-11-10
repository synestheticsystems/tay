//! How a container ended, as Docker says: its exit code, when, whether it
//! was killed for memory, and what went wrong, if anything did.

use bollard::models::ContainerInspectResponse;

use crate::clock;

/// How a container ended, as Docker says.
pub struct Ended {
    code: i64,
    out_of_memory: bool,
    /// When, in milliseconds since 1970, when Docker says.
    at: Option<u64>,
    /// What Docker says went wrong, if anything did.
    error: Option<String>,
}

impl Ended {
    pub fn of(inspected: &ContainerInspectResponse) -> Ended {
        let state = inspected.state.as_ref();
        Ended {
            code: state.and_then(|state| state.exit_code).unwrap_or_default(),
            out_of_memory: state.and_then(|state| state.oom_killed).unwrap_or_default(),
            at: state
                .and_then(|state| state.finished_at.as_deref())
                .and_then(clock::nanos_of)
                .map(|nanos| nanos / 1_000_000),
            error: state
                .and_then(|state| state.error.clone())
                .filter(|error| !error.is_empty()),
        }
    }

    pub fn succeeded(&self) -> bool {
        self.code == 0 && !self.out_of_memory
    }

    /// Its exit status, as a shell gives it: Docker gives 128 and the
    /// number of the signal that ended it, as a shell does.
    pub fn exit_status(&self) -> i64 {
        self.code
    }

    /// How it ended, as its log notes it.
    pub fn how(&self) -> String {
        let mut how = format!("exited with {}", self.code);
        if self.out_of_memory {
            how.push_str(", killed for memory");
        }
        if let Some(error) = &self.error {
            how.push_str(&format!(": {error}"));
        }
        how
    }

    /// How it ended, as a person reads it after "attempt 2", with the time.
    pub fn words(&self) -> String {
        let mut words = format!("exited with {}", self.code);
        if let Some(at) = self.at {
            words.push_str(&format!(" at {}", clock::utc(at / 1000)));
        }
        if self.out_of_memory {
            words.push_str(", killed for memory");
        }
        if let Some(error) = &self.error {
            words.push_str(&format!(": {error}"));
        }
        words
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn how_a_container_ended_says_its_code_its_time_and_whether_it_was_killed_for_memory() {
        let ended = |code, out_of_memory, error: Option<&str>| Ended {
            code,
            out_of_memory,
            at: Some(1_794_292_971_005),
            error: error.map(str::to_string),
        };
        assert_eq!(
            ended(3, false, None).words(),
            "exited with 3 at 2026-11-10 06:42:51 UTC"
        );
        assert!(!ended(3, false, None).succeeded());
        let killed = ended(137, true, None);
        assert_eq!(
            killed.words(),
            "exited with 137 at 2026-11-10 06:42:51 UTC, killed for memory"
        );
        assert_eq!(killed.how(), "exited with 137, killed for memory");
        // A program that exits with 0 after something of it was killed for
        // memory did not do what it was run for.
        assert!(!ended(0, true, None).succeeded());
        assert!(ended(0, false, None).succeeded());
        assert_eq!(
            ended(127, false, Some("exec: \"x\": not found")).how(),
            "exited with 127: exec: \"x\": not found"
        );
    }
}
