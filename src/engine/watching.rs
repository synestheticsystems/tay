//! Watching, in development: a unit with `watch` is run again when a file
//! under the paths it names changes, files that git ignores excepted. The
//! system says what changed, through the `notify` crate: FSEvents on macOS
//! and inotify on Linux, so nothing is looked at while nothing changes.
//!
//! The system may tell of a change made just before it was asked to
//! watch, so a file that is there and was last changed before then has not
//! changed. Changes are gathered for a moment, so that saving several files
//! runs the unit once. A unit that is starting when they come is not stopped:
//! it runs once more when it has settled, however many changes came
//! meanwhile. A unit that has ended, as a command that is done or failed
//! has, is run again; one that runs, as a process does, is started again.
//! While a command runs again, and if it fails, the units that use what
//! it builds are left as they are; once it is done, each of them is
//! started again if what it would be started from now differs from what
//! it was started from, and left alone if not.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use super::started_from;
use super::state::{State, UnitState};
use super::supervisor::{Begin, Supervisor};
use crate::clock;
use crate::diagnostic::quoted_list;
use crate::disk::unit_log::{UnitLog, Writer};
use crate::module::Operations;
use crate::plan::PlannedUnit;

/// How long changes are gathered after the last of them before the unit
/// runs again.
const GATHER: Duration = Duration::from_millis(300);

/// How many of the files that changed are named.
const NAMED: usize = 3;

/// How many of the last lines a command that failed when it ran again
/// wrote are said with its failure.
const LAST_SAID: usize = 3;

/// What the system says of the files under a unit's watch.
type Said = notify::Result<Event>;

/// Begins to watch the files `planned` names under `watch`, telling the
/// task it gives each change, which it runs the unit again for; why not,
/// when they cannot be watched. The watch ends when the task does.
pub fn watch(
    supervisor: &Arc<Supervisor>,
    planned: &Arc<PlannedUnit>,
) -> Result<impl Future<Output = ()> + Send + 'static, String> {
    let given = supervisor.given();
    let mut paths = Vec::new();
    for watched in planned
        .mechanism
        .iter()
        .flat_map(|mechanism| &mechanism.watch)
    {
        let path = watched
            .fill(&mut |placeholder| supervisor.fill(placeholder, &given))
            .map_err(|why| why.to_string())?;
        paths.push(PathBuf::from(path.reveal()));
    }
    let since = SystemTime::now();
    let (tell, told) = mpsc::unbounded_channel();
    // The system's thread tells the task, which has ended when it cannot
    // be told.
    let mut watcher = notify::recommended_watcher(move |said: Said| {
        let _ = tell.send(said);
    })
    .map_err(|error| format!("its files cannot be watched: {error}"))?;
    for path in &paths {
        watcher
            .watch(path, RecursiveMode::Recursive)
            .map_err(|error| format!("`{}` cannot be watched: {error}", path.display()))?;
    }
    let watching = Watching {
        supervisor: Arc::clone(supervisor),
        planned: Arc::clone(planned),
        paths,
        since,
        told,
        _watcher: watcher,
    };
    Ok(watching.run())
}

/// One unit's files being watched.
struct Watching {
    supervisor: Arc<Supervisor>,
    planned: Arc<PlannedUnit>,
    /// The files and folders its `watch` names.
    paths: Vec<PathBuf>,
    /// When watching them began.
    since: SystemTime,
    told: mpsc::UnboundedReceiver<Said>,
    /// Watches for as long as it is held.
    _watcher: RecommendedWatcher,
}

impl Watching {
    async fn run(mut self) {
        while let Some(first) = self.told.recv().await {
            let mut gathered = Gathered::default();
            gathered.take(first);
            loop {
                match tokio::time::timeout(GATHER, self.told.recv()).await {
                    Ok(Some(said)) => gathered.take(said),
                    Ok(None) => return,
                    Err(_) => break,
                }
            }
            if let Some(because) = gathered.because(&self).await {
                run_again(&self.supervisor, &self.planned, &because).await;
            }
        }
    }

    /// Whether `path` changed since watching began: it is a file changed
    /// since, or it is gone. A folder changes as files in it do, which are
    /// told of themselves.
    fn changed_since(&self, path: &Path) -> bool {
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => false,
            Ok(metadata) => metadata
                .modified()
                .is_ok_and(|modified| modified >= self.since),
            Err(_) => true,
        }
    }

    /// The path `path` is shown as: from the stack's root when it is
    /// under it.
    fn shown(&self, path: &Path) -> String {
        let root = self.supervisor.environment.layout.root();
        path.strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string()
    }
}

/// What the system said while changes were gathered: the files that
/// changed, and what went wrong with watching them.
#[derive(Default)]
struct Gathered {
    paths: BTreeSet<PathBuf>,
    problems: Vec<String>,
}

impl Gathered {
    fn take(&mut self, said: Said) {
        match said {
            // A file read is not changed.
            Ok(event) if matches!(event.kind, EventKind::Access(_)) => {}
            // What git keeps of a repository is none of its files.
            Ok(event) => self.paths.extend(
                (event.paths.into_iter())
                    .filter(|path| !path.components().any(|part| part.as_os_str() == ".git")),
            ),
            // The system may have missed changes, so they are taken to have
            // come.
            Err(error) => self.problems.push(format!("watching its files: {error}")),
        }
    }

    /// What changed, as a person reads it, once what git ignores is left
    /// out; `None` when nothing that counts did.
    async fn because(self, watching: &Watching) -> Option<String> {
        let mut problems = self.problems;
        let mut changed = Vec::new();
        for folder in &watching.paths {
            let under: Vec<PathBuf> = (self.paths.iter())
                .filter(|path| path.starts_with(folder) && watching.changed_since(path))
                .cloned()
                .collect();
            if under.is_empty() {
                continue;
            }
            let folder = match folder.is_dir() {
                true => folder.as_path(),
                false => folder.parent().unwrap_or(folder),
            };
            match not_ignored(folder, under.clone()).await {
                Ok(counted) => changed.extend(counted),
                Err(why) => {
                    problems.push(format!("which files git ignores cannot be told: {why}"));
                    changed.extend(under);
                }
            }
        }
        changed.sort();
        changed.dedup();
        if changed.is_empty() && problems.is_empty() {
            return None;
        }
        let mut said = Vec::new();
        if !changed.is_empty() {
            let names: Vec<String> = (changed.iter().take(NAMED))
                .map(|path| watching.shown(path))
                .collect();
            let more = match changed.len().saturating_sub(NAMED) {
                0 => String::new(),
                more => format!(" and {more} more"),
            };
            said.push(format!("{}{more} changed", quoted_list(&names)));
        }
        said.extend(problems);
        Some(said.join("; "))
    }
}

/// Of `paths`, those git does not ignore, asked of git in `folder`: every
/// one of them where `folder` is in no repository of git's.
async fn not_ignored(folder: &Path, paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let git = || {
        let mut git = tokio::process::Command::new("git");
        git.arg("-C").arg(folder).stderr(Stdio::piped());
        git
    };
    let inside = git()
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(Stdio::piped())
        .output()
        .await
        .map_err(|error| format!("git cannot be run: {error}"))?;
    if !inside.status.success() {
        return Ok(paths);
    }
    let mut asking = git()
        .args(["check-ignore", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("git cannot be run: {error}"))?;
    let mut asked = Vec::new();
    for path in &paths {
        asked.extend_from_slice(path.as_os_str().as_encoded_bytes());
        asked.push(0);
    }
    let mut stdin = asking.stdin.take().expect("its standard input is piped");
    stdin
        .write_all(&asked)
        .await
        .map_err(|error| format!("git cannot be asked: {error}"))?;
    drop(stdin);
    let answer = asking
        .wait_with_output()
        .await
        .map_err(|error| format!("git cannot be asked: {error}"))?;
    // It exits with 1 when it ignores none of them.
    if !matches!(answer.status.code(), Some(0 | 1)) {
        return Err(String::from_utf8_lossy(&answer.stderr).trim().to_string());
    }
    let ignored: BTreeSet<&[u8]> = answer
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .collect();
    Ok(paths
        .into_iter()
        .filter(|path| !ignored.contains(path.as_os_str().as_encoded_bytes()))
        .collect())
}

/// Runs `planned` again because `because` changed, once it has settled:
/// one that has ended is run again, and one that runs is started again.
/// Once it is done, each unit that uses a file it builds is started again
/// if what it would be started from has changed. One that is stopped, as
/// a unit is that was not brought up or that a change stopped, is left
/// stopped, and one that waits is left waiting: none is started against a
/// need that is not ready.
async fn run_again(supervisor: &Arc<Supervisor>, planned: &Arc<PlannedUnit>, because: &str) {
    let unit = &planned.name;
    settled(supervisor, unit, Until::Settled).await;
    let standing = supervisor.state(unit).map(|state| state.state);
    let needs_ready = (planned.needs.iter()).all(|need| {
        supervisor
            .state(&need.unit)
            .is_some_and(|state| state.state == State::Ready)
    });
    if matches!(standing, None | Some(State::Stopped | State::Waiting)) || !needs_ready {
        return;
    }
    let Some(running) = operations(supervisor, planned) else {
        return;
    };
    if supervisor.is_tended(unit)
        && let Err(why) = supervisor.stop_to_replace(planned).await
    {
        let reason = format!("{because}, and it could not be stopped to start it again: {why}");
        supervisor.record(unit, UnitState::new(State::Failed, reason), None);
        return;
    }
    let began = clock::stamp(clock::now_millis());
    supervisor.tend(planned, running, Begin::Again(because.to_string()), true);
    // What builds files other units use, as a command does, is done once
    // its run has ended, ready.
    let users = users_of(supervisor, unit);
    let until = match users.is_empty() {
        true => Until::Settled,
        false => Until::Ended,
    };
    settled(supervisor, unit, until).await;
    let Some(after) = supervisor.state(unit) else {
        return;
    };
    match after.state {
        State::Ready if !supervisor.is_tended(unit) => {
            start_again_if_changed(supervisor, users).await
        }
        State::Failed => {
            let said = last_said(supervisor, unit, &began);
            if !said.is_empty() {
                let detail = format!("{}; it last wrote: {said}", after.detail);
                supervisor.record(unit, UnitState::new(State::Failed, detail), None);
            }
        }
        _ => {}
    }
}

/// The operations of `planned`, filled from what the units that are ready
/// give now; `None` when it cannot be filled, which fails it.
fn operations(
    supervisor: &Supervisor,
    planned: &PlannedUnit,
) -> Option<Arc<dyn crate::module::Running>> {
    let mechanism = planned.mechanism.as_ref()?;
    let given = supervisor.given();
    match mechanism.operations(&mut |placeholder| supervisor.fill(placeholder, &given)) {
        Ok(Operations::Running(running)) => Some(running),
        Ok(Operations::Made(_)) => None,
        Err(why) => {
            let reason = why.to_string();
            supervisor.record(&planned.name, UnitState::new(State::Failed, reason), None);
            None
        }
    }
}

/// Each unit that uses a file `unit` writes or builds.
fn users_of(supervisor: &Supervisor, unit: &str) -> Vec<Arc<PlannedUnit>> {
    supervisor
        .snapshot()
        .iter()
        .map(|known| Arc::clone(&known.planned))
        .filter(|planned| {
            (planned.mechanism.iter())
                .flat_map(|mechanism| &mechanism.files)
                .any(|file| file.of == unit)
        })
        .collect()
}

/// Starts again each of `users` that runs, if what it would be started
/// from now differs from what it was started from.
async fn start_again_if_changed(supervisor: &Arc<Supervisor>, users: Vec<Arc<PlannedUnit>>) {
    let running = users
        .into_iter()
        .filter(|user| supervisor.is_tended(&user.name));
    for user in running {
        let Some(running) = operations(supervisor, &user) else {
            continue;
        };
        let why = match started_from::changed(supervisor, &user, Arc::clone(&running)).await {
            Ok(Some(why)) => why,
            Ok(None) => continue,
            Err(why) => format!("what it would be started from cannot be told: {why}"),
        };
        let again = UnitState::new(State::Starting, format!("{why}, so it is started again"));
        supervisor.record(&user.name, again, None);
        match supervisor.stop_to_replace(&user).await {
            Ok(()) => supervisor.tend(&user, running, Begin::Fresh, true),
            Err(stopped) => {
                let reason =
                    format!("{why}, and it could not be stopped to start it again: {stopped}");
                supervisor.record(&user.name, UnitState::new(State::Failed, reason), None);
            }
        }
    }
}

/// How far a unit is waited for.
enum Until {
    /// No task looks after it while it is starting, or waiting to.
    Settled,
    /// No task looks after it.
    Ended,
}

/// Waits for `unit` until it is as `until` says.
async fn settled(supervisor: &Supervisor, unit: &str, until: Until) {
    let mut changes = supervisor.changes();
    loop {
        let state = supervisor.state(unit).map(|state| state.state);
        let settling = match until {
            Until::Settled => matches!(state, Some(State::Starting | State::Waiting)),
            Until::Ended => true,
        };
        if !(settling && supervisor.is_tended(unit)) {
            return;
        }
        if changes.changed().await.is_err() {
            return;
        }
    }
}

/// The last lines `unit` wrote itself since the time `began`, as a person
/// reads them.
fn last_said(supervisor: &Supervisor, unit: &str, began: &str) -> String {
    let log = UnitLog::at(supervisor.environment.layout.unit_log(unit));
    let lines = match log.read_since(began) {
        Ok((lines, _)) => lines,
        Err(error) => return format!("its log cannot be read: {error}"),
    };
    let said: Vec<String> = (lines.into_iter())
        .filter(|line| line.writer == Writer::Unit)
        .map(|line| line.text)
        .collect();
    quoted_list(&said[said.len().saturating_sub(LAST_SAID)..])
}
