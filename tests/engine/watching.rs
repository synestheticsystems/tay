//! Watching, in development: a unit with `watch` is run again when a file
//! under the paths it names changes, files that git ignores excepted. A
//! command runs again and a process is started again; changes saved
//! together run it once, and changes that come while it runs run it once
//! more. While a command runs again, and if it fails, the units that use
//! what it builds are left as they are; once it is done, each is started
//! again only if the file it uses changed.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::harness::{Machine, child, eventually, log};

/// Long enough for changes to have been gathered and run for, on a loaded
/// machine, when nothing is to come of them.
const QUIET: Duration = Duration::from_secs(3);

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

/// The stack `watching`, brought up, with the build run once.
fn brought_up(machine: &Machine, units: &[&str]) -> std::path::PathBuf {
    let root = machine.stack("watching");
    repository(&root);
    let up = machine.at(&root, "up", units);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    root
}

#[test]
fn files_saved_together_run_a_command_once_and_files_git_ignores_not_at_all() {
    let machine = Machine::new();
    let root = brought_up(&machine, &[]);
    assert_eq!(runs(&root), 1);
    fs::create_dir(root.join("src/ignored")).unwrap();
    fs::write(root.join("src/ignored/notes.txt"), "noted\n").unwrap();
    std::thread::sleep(QUIET);
    assert_eq!(runs(&root), 1);
    for (file, says) in [("one", "uno"), ("two", "dos"), ("three", "tres")] {
        fs::write(root.join(format!("src/{file}.txt")), format!("{says}\n")).unwrap();
    }
    said(&root, "first", "| one says uno");
    said(&root, "second", "| two says dos");
    std::thread::sleep(QUIET);
    assert_eq!(runs(&root), 2);
}

#[test]
fn once_a_build_is_done_only_what_uses_a_file_it_changed_is_started_again() {
    let machine = Machine::new();
    let root = brought_up(&machine, &[]);
    said(&root, "second", "| two says two");
    let second = child(&root, "second");
    fs::write(root.join("src/one.txt"), "uno\n").unwrap();
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
    fs::remove_file(root.join("src/fail")).unwrap();
    machine.until_state(&root, "build", Duration::from_secs(30), |state| {
        state.starts_with("ready")
    });
    assert_eq!(runs(&root), 3);
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
    std::thread::sleep(Duration::from_millis(500));
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
    said(&root, "ticker", "| ticks 2");
    assert_ne!(child(&root, "ticker"), first);
}
