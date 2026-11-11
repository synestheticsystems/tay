//! Watching, in development: a unit with `watch` is run again when a file
//! under the paths it names changes, files that git ignores excepted. A
//! command runs again and a process is started again; changes saved
//! together run it once, and changes that come while it runs run it once
//! more. While a command runs again, and if it fails, the units that use
//! what it builds are left as they are; once it is done, each is started
//! again only if the file it uses changed. On Linux, where the system
//! watches one folder at a time, a folder git ignores is not watched at
//! all.
//!
//! The system tells of a saved file when it can, which on a loaded machine
//! may be many seconds later, and may tell of files saved together in more
//! than one go. So a test waits on what tay says it took in: each run
//! again is said in the daemon's log with the files it was for, and the
//! build is to have run once for each, never more.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::harness::{Machine, child, eventually, log};

/// Long enough for changes to have been gathered and run for, on a loaded
/// machine, when nothing is to come of them.
const QUIET: Duration = Duration::from_secs(3);

/// How long the system may take to tell of a saved file, however loaded
/// the machine is, before a test gives up on tay ever taking it in.
const TOLD: Duration = Duration::from_secs(180);

/// Makes the stack at `root` a repository of git's, which ignores
/// `src/ignored/`.
fn repository(root: &Path) {
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .expect("git runs");
    assert!(init.success());
    fs::write(root.join(".gitignore"), "src/ignored/\n").unwrap();
}

/// How many times the build has run.
fn runs(root: &Path) -> usize {
    fs::read_to_string(root.join(".tay/state/runs"))
        .unwrap_or_default()
        .lines()
        .count()
}

/// Waits until `unit`'s log has a line with `said`.
fn said(root: &Path, unit: &str, said: &str) {
    assert!(
        eventually(Duration::from_secs(30), || log(root, unit).contains(said)),
        "`{unit}` said `{said}`: {}",
        log(root, unit)
    );
}

/// For each time tay ran `unit` again for files that changed, in order,
/// the files it named, as its daemon's log says it.
fn ran_again_for(root: &Path, unit: &str) -> Vec<Vec<String>> {
    let log = fs::read_to_string(root.join(".tay/logs/tay/daemon.log")).unwrap_or_default();
    let again = format!("  {unit}: starting (attempt 1, starting; ");
    log.lines()
        .filter_map(|line| line.split_once(&again))
        .map(|(_, because)| {
            (because.split('`').skip(1).step_by(2))
                .map(str::to_string)
                .collect()
        })
        .collect()
}

/// Waits until tay says it has run `unit` again for each of `files`,
/// however late the system tells it of them.
fn taken_in(root: &Path, unit: &str, files: &[&str]) {
    let mut again = Vec::new();
    let taken = eventually(TOLD, || {
        again = ran_again_for(root, unit);
        let named = || again.iter().flatten();
        files.iter().all(|file| named().any(|named| named == file))
    });
    assert!(taken, "`{unit}` ran again for each of {files:?}: {again:?}");
}

/// Waits until the build is ready having run once as it was brought up
/// and once for each time tay said it ran it again, and gives those: it
/// fails a test in which the build ran more often than tay said.
fn ran_once_for_each(machine: &Machine, root: &Path) -> Vec<Vec<String>> {
    let (mut again, mut ran) = (Vec::new(), 0);
    let settled = eventually(TOLD, || {
        again = ran_again_for(root, "build");
        ran = runs(root);
        ran == 1 + again.len() && {
            let status = machine.at(root, "status", &["--json"]).json();
            Machine::state(&status, "build").starts_with("ready")
        }
    });
    assert!(
        settled,
        "the build ran {ran} times: once as it was brought up, and once for each of {again:?}"
    );
    again
}

/// The stack `watching`, brought up, with the build run once.
fn brought_up(machine: &Machine, units: &[&str]) -> std::path::PathBuf {
    let root = machine.stack("watching");
    repository(&root);
    let up = machine.at(&root, "up", units);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    root
}

#[test]
fn files_saved_together_run_a_command_once_for_what_it_takes_in_and_files_git_ignores_not_at_all() {
    let machine = Machine::new();
    let root = brought_up(&machine, &[]);
    assert_eq!(runs(&root), 1);
    // The file git ignores is saved first: the system tells of changes in
    // the order they are made, so by the time tay has taken in those saved
    // after it, it has judged that one too.
    fs::create_dir(root.join("src/ignored")).unwrap();
    fs::write(root.join("src/ignored/notes.txt"), "noted\n").unwrap();
    for (file, says) in [("one", "uno"), ("two", "dos"), ("three", "tres")] {
        fs::write(root.join(format!("src/{file}.txt")), format!("{says}\n")).unwrap();
    }
    taken_in(
        &root,
        "build",
        &["src/one.txt", "src/two.txt", "src/three.txt"],
    );
    said(&root, "first", "| one says uno");
    said(&root, "second", "| two says dos");
    let again = ran_once_for_each(&machine, &root);
    let ignored = again
        .iter()
        .flatten()
        .filter(|file| file.contains("ignored"));
    assert_eq!(ignored.count(), 0, "{again:?}");
}

#[test]
fn once_a_build_is_done_only_what_uses_a_file_it_changed_is_started_again() {
    let machine = Machine::new();
    let root = brought_up(&machine, &[]);
    said(&root, "second", "| two says two");
    let second = child(&root, "second");
    fs::write(root.join("src/one.txt"), "uno\n").unwrap();
    taken_in(&root, "build", &["src/one.txt"]);
    said(&root, "first", "| one says uno");
    let daemon = fs::read_to_string(root.join(".tay/logs/tay/daemon.log")).unwrap();
    assert!(
        daemon.contains(
            "first: starting (the file `build.outputs.one` changed, so it is started again)"
        ),
        "{daemon}"
    );
    assert!(!daemon.contains("second: starting (the file"), "{daemon}");
    assert_eq!(child(&root, "second"), second);
}

#[test]
fn a_build_that_fails_says_its_last_lines_and_leaves_what_uses_it_on_the_last_good_one() {
    let machine = Machine::new();
    let root = brought_up(&machine, &[]);
    said(&root, "first", "| one says one");
    let first = child(&root, "first");
    fs::write(root.join("src/fail"), "told to\n").unwrap();
    taken_in(&root, "build", &["src/fail"]);
    let failed = machine.until_state(&root, "build", Duration::from_secs(30), |state| {
        state.starts_with("failed")
    });
    assert!(
        failed.ends_with("; it last wrote: `cannot build: told to`"),
        "{failed}"
    );
    let status = machine.at(&root, "status", &["--json"]).json();
    assert!(Machine::state(&status, "first").starts_with("ready"));
    assert_eq!(child(&root, "first"), first);
    // The next change runs it again.
    let failing = ran_again_for(&root, "build").len();
    fs::remove_file(root.join("src/fail")).unwrap();
    assert!(
        eventually(TOLD, || ran_again_for(&root, "build").len() > failing),
        "the build ran again"
    );
    ran_once_for_each(&machine, &root);
}

#[test]
fn changes_while_a_command_runs_again_run_it_once_more_and_it_says_what_changed() {
    let machine = Machine::new();
    let root = brought_up(&machine, &[]);
    fs::write(root.join(".tay/state/hold"), "").unwrap();
    fs::write(root.join("src/one.txt"), "uno\n").unwrap();
    let running = machine.until_state(&root, "build", Duration::from_secs(30), |state| {
        state.starts_with("starting") && state.contains("running")
    });
    assert!(running.ends_with("; `src/one.txt` changed"), "{running}");
    said(&root, "build", "| holding");
    fs::write(root.join("src/two.txt"), "dos\n").unwrap();
    std::thread::sleep(Duration::from_millis(500));
    fs::write(root.join("src/three.txt"), "tres\n").unwrap();
    // The system tells of both while the build holds, however loaded the
    // machine is.
    std::thread::sleep(QUIET);
    fs::write(root.join(".tay/state/go"), "").unwrap();
    said(&root, "second", "| two says dos");
    std::thread::sleep(QUIET);
    assert_eq!(runs(&root), 3);
}

#[test]
fn a_watched_process_is_started_again_when_its_file_changes() {
    let machine = Machine::new();
    let root = brought_up(&machine, &["ticker"]);
    said(&root, "ticker", "| ticks 1");
    let first = child(&root, "ticker");
    fs::write(root.join("src/ticker.txt"), "2\n").unwrap();
    taken_in(&root, "ticker", &["src/ticker.txt"]);
    said(&root, "ticker", "| ticks 2");
    assert_ne!(child(&root, "ticker"), first);
}

/// The folders and files the daemon of the environment at `root` asks the
/// system to watch, by inode, as the system lists each watch.
#[cfg(target_os = "linux")]
fn watched(machine: &Machine, root: &Path) -> std::collections::BTreeSet<u64> {
    let pid = machine.daemon_pid(root).expect("a daemon answers");
    let mut inodes = std::collections::BTreeSet::new();
    for entry in fs::read_dir(format!("/proc/{pid}/fdinfo")).unwrap() {
        let info = fs::read_to_string(entry.unwrap().path()).unwrap_or_default();
        let watches = info.lines().filter(|line| line.starts_with("inotify wd:"));
        for watch in watches {
            let inode = watch
                .split_whitespace()
                .find_map(|word| word.strip_prefix("ino:"))
                .expect("a watch names its inode");
            inodes.insert(u64::from_str_radix(inode, 16).unwrap());
        }
    }
    inodes
}

#[cfg(target_os = "linux")]
#[test]
fn on_linux_a_folder_git_ignores_is_not_watched_and_one_made_later_is() {
    use std::os::unix::fs::MetadataExt;
    let machine = Machine::new();
    let root = machine.stack("watching");
    repository(&root);
    fs::create_dir_all(root.join("src/ignored/deep/deeper")).unwrap();
    fs::create_dir_all(root.join("src/kept/deep")).unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let inode = |folder: &str| fs::metadata(root.join(folder)).unwrap().ino();
    let watched_now = watched(&machine, &root);
    for folder in ["src", "src/kept", "src/kept/deep"] {
        assert!(
            watched_now.contains(&inode(folder)),
            "`{folder}` is watched"
        );
    }
    for folder in ["src/ignored", "src/ignored/deep", "src/ignored/deep/deeper"] {
        assert!(
            !watched_now.contains(&inode(folder)),
            "`{folder}` is not watched"
        );
    }
    // A folder made later is watched once it is seen, and a file saved in
    // it runs the build; one made where git ignores is not.
    fs::create_dir_all(root.join("src/later/deep")).unwrap();
    fs::create_dir(root.join("src/ignored/later")).unwrap();
    assert!(
        eventually(Duration::from_secs(30), || {
            watched(&machine, &root).contains(&inode("src/later/deep"))
        }),
        "`src/later/deep` is watched"
    );
    assert!(!watched(&machine, &root).contains(&inode("src/ignored/later")));
    assert_eq!(runs(&root), 1);
    fs::write(root.join("src/later/deep/notes.txt"), "noted\n").unwrap();
    taken_in(&root, "build", &["src/later/deep/notes.txt"]);
    ran_once_for_each(&machine, &root);
}
