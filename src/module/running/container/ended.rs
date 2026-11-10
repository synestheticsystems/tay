//! How a container ended, as Docker says: its exit code, when, whether it
//! was killed for memory, and what went wrong, if anything did.

use bollard::models::ContainerInspectResponse;

use crate::clock;

/// The exit code of a container whose program was killed outright, as a
/// memory kill does: 128 and the number of `SIGKILL`.
const KILLED: i64 = 128 + 9;

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

    /// When it started, in nanoseconds since 1970, if it was killed by
    /// the signal a memory kill sends and Docker has not yet said it was
    /// one: Docker learns of a memory kill apart from the end.
    pub fn killed_since(inspected: &ContainerInspectResponse) -> Option<u64> {
        let state = inspected.state.as_ref()?;
        let unsaid = state.exit_code == Some(KILLED) && state.oom_killed != Some(true);
        unsaid
            .then(|| state.started_at.as_deref().and_then(clock::nanos_of))
            .flatten()
    }

    /// Takes in that Docker has since said it was killed for memory.
    pub fn killed_for_memory(&mut self) {
        self.out_of_memory = true;
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

    #[test]
    fn a_container_killed_outright_is_asked_about_memory_only_while_docker_has_not_said() {
        let inspected = |code, oom_killed| ContainerInspectResponse {
            state: Some(bollard::models::ContainerState {
                exit_code: Some(code),
                oom_killed: Some(oom_killed),
                started_at: Some("2026-11-10T06:42:51.000000005Z".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            Ended::killed_since(&inspected(137, false)),
            Some(1_794_292_971_000_000_005)
        );
        // Docker has said it already, or it was not killed outright.
        assert_eq!(Ended::killed_since(&inspected(137, true)), None);
        assert_eq!(Ended::killed_since(&inspected(143, false)), None);
        assert_eq!(Ended::killed_since(&inspected(1, false)), None);
    }
}
