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
//! quiet for that long, and when the copying is dropped, as it is to stop
//! the container; so only a daemon that is killed copies again, at most what
//! was printed in that while.

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
    let mut copying = Copying {
        kept: Copied::of(copied, container)?,
        path: copied,
        last_kept: None,
        unkept: false,
    };
    let since = i32::try_from(copying.kept.at / 1_000_000_000).unwrap_or(i32::MAX);
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
    loop {
        let said = tokio::select! {
            said = output.next() => said,
            () = tokio::time::sleep_until(copying.next_keep().into()), if copying.unkept => {
                copying.keep()?;
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
            if copying.kept.has(at, nth) {
                continue;
            }
            log.write_at(at / 1_000_000, Writer::Unit, text.trim_end_matches('\r'))?;
            (copying.kept.at, copying.kept.lines) = (at, nth);
            copying.unkept = true;
        }
        if copying.unkept && Instant::now() >= copying.next_keep() {
            copying.keep()?;
        }
    }
    if copying.unkept {
        copying.keep()?;
    }
    Ok(())
}

/// A copying under way: where it has come to, and whether that is kept.
struct Copying<'a> {
    kept: Copied,
    path: &'a Path,
    /// When where it had come to was last kept.
    last_kept: Option<Instant>,
    /// Whether lines have been copied since.
    unkept: bool,
}

impl Copying<'_> {
    /// When where it has come to is kept next: at once, unless it was kept
    /// a moment ago, and then once that moment has passed.
    fn next_keep(&self) -> Instant {
        self.last_kept.map_or_else(Instant::now, |last| last + KEEP)
    }

    fn keep(&mut self) -> Result<(), DiskError> {
        self.kept.keep(self.path)?;
        (self.last_kept, self.unkept) = (Some(Instant::now()), false);
        Ok(())
    }
}

impl Drop for Copying<'_> {
    /// A copying dropped before it ends, as one is to stop its container,
    /// keeps where it came to, so that the next copies on from there. If
    /// that cannot be written, the next copies again at most a moment's
    /// lines, which is all that is lost by not saying so.
    fn drop(&mut self) {
        if self.unkept {
            let _ = self.keep();
        }
    }
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
