//! A container's output, copied into its unit's log with the time Docker
//! kept for each line, from where the copying last stopped. Docker holds
//! the output, so a daemon that was away loses nothing Docker still has;
//! and before a container is removed, what remains of it is copied.
//!
//! Where the copying stopped is kept beside the unit's other files: the
//! container, the time of the last line copied, to the nanosecond, and how
//! many lines of that very time were copied, since Docker gives output only
//! from a whole second on. It is written as each line is copied, but at
//! most every so often while a container prints, and then once it has gone
//! quiet for that long; so copying that stops, as when the daemon is killed
//! or the copying is dropped to stop the container, copies again at most
//! what was printed in that while.

use std::path::Path;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use bollard::container::LogOutput;
use bollard::query_parameters::LogsOptionsBuilder;

use crate::clock;
use crate::disk::unit_log::{Adding, Writer};
use crate::disk::{DiskError, write_whole};
use crate::module::running::container::docker::ours::OurContainer;
use crate::module::running::container::docker::{Docker, DockerError, missing};

/// How often, at most, where the copying has come to is written down while
/// a container prints.
const KEEP: Duration = Duration::from_millis(250);

/// Where the copying of a unit's container output stopped.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Copied {
    /// The container it was copied from.
    container: String,
    /// The time Docker kept for the last line copied, in nanoseconds since
    /// 1970.
    at: u64,
    /// How many lines of that time were copied.
    lines: u64,
}

impl Copied {
    /// What was copied of `container`, as kept at `path`: nothing, when
    /// what is kept is of another container, or nothing is.
    fn of(path: &Path, container: &OurContainer) -> Result<Copied, DiskError> {
        let fresh = Copied {
            container: container.id.clone(),
            ..Copied::default()
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(fresh),
            Err(error) => return Err(DiskError::io(path, "read")(error)),
        };
        let kept: Copied = serde_json::from_str(&text).map_err(|source| DiskError::Unreadable {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(match kept.container == container.id {
            true => kept,
            false => fresh,
        })
    }

    fn keep(&self, path: &Path) -> Result<(), DiskError> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(DiskError::io(folder, "made"))?;
        }
        let json = serde_json::to_string(self).expect("where copying stopped serializes");
        write_whole(path, format!("{json}\n").as_bytes(), 0o644)
    }

    /// Whether the line at `at`, the `nth` of that time this copying has
    /// read, was copied before.
    fn has(&self, at: u64, nth: u64) -> bool {
        at < self.at || (at == self.at && nth <= self.lines)
    }
}

/// Copies what `container` wrote that has not been copied into `log`, as
/// `copied` keeps track: all that Docker holds now, or with `follow`, on
/// until the container stops.
pub async fn copy_output(
    docker: &Docker,
    container: &OurContainer,
    log: &mut Adding,
    copied: &Path,
    follow: bool,
) -> Result<(), DockerError> {
    let disk = |error: DiskError| DockerError::Other(error.to_string());
    let mut kept = Copied::of(copied, container).map_err(disk)?;
    let since = i32::try_from(kept.at / 1_000_000_000).unwrap_or(i32::MAX);
    let options = LogsOptionsBuilder::default()
        .follow(follow)
        .stdout(true)
        .stderr(true)
        .timestamps(true)
        .since(since)
        .build();
    let mut output = docker.api.logs(&container.id, Some(options));
    // The time of the line read last, and how many lines of it were read.
    let (mut at, mut nth) = (0, 0);
    // When where the copying has come to was last kept, and whether lines
    // have been copied since.
    let mut last_kept: Option<Instant> = None;
    let mut unkept = false;
    loop {
        // Where the copying has come to is kept at once, unless it was kept
        // a moment ago; then once that moment has passed, whether or not
        // more lines come.
        let next_keep = last_kept.map_or_else(Instant::now, |last| last + KEEP);
        let said = tokio::select! {
            said = output.next() => said,
            () = tokio::time::sleep_until(next_keep.into()), if unkept => {
                kept.keep(copied).map_err(disk)?;
                (last_kept, unkept) = (Some(Instant::now()), false);
                continue;
            }
        };
        let Some(said) = said else {
            break;
        };
        let message = match said {
            Ok(
                LogOutput::StdOut { message }
                | LogOutput::StdErr { message }
                | LogOutput::Console { message },
            ) => message,
            Ok(LogOutput::StdIn { .. }) => continue,
            // A container removed meanwhile has no more to give.
            Err(error) if missing(&error) => break,
            Err(error) => {
                return Err(docker.failed(format!(
                    "read what the container of `{}` wrote",
                    container.unit()
                ))(error));
            }
        };
        for line in String::from_utf8_lossy(&message).split_terminator('\n') {
            let (stamp, text) = line.split_once(' ').unwrap_or((line, ""));
            // Every line Docker gives with times begins with one; one that
            // does not is taken to be of the time before it.
            match clock::nanos_of(stamp) {
                Some(time) if time == at => nth += 1,
                Some(time) => (at, nth) = (time, 1),
                None => nth += 1,
            }
            if kept.has(at, nth) {
                continue;
            }
            log.write_at(at / 1_000_000, Writer::Unit, text.trim_end_matches('\r'))
                .map_err(disk)?;
            (kept.at, kept.lines) = (at, nth);
            unkept = true;
        }
        if unkept && last_kept.is_none_or(|last| last.elapsed() >= KEEP) {
            kept.keep(copied).map_err(disk)?;
            (last_kept, unkept) = (Some(Instant::now()), false);
        }
    }
    if unkept {
        kept.keep(copied).map_err(disk)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_was_copied_if_it_is_older_or_one_of_the_first_of_its_time() {
        let kept = Copied {
            container: "c".to_string(),
            at: 10,
            lines: 2,
        };
        assert!(kept.has(9, 5));
        assert!(kept.has(10, 2));
        assert!(!kept.has(10, 3));
        assert!(!kept.has(11, 1));
        assert!(!Copied::default().has(1, 1));
    }
}
