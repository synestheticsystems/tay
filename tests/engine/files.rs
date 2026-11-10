//! `file` units: written whole and then moved into place, written again
//! only when the content would differ, never when `overwrite` is false and
//! the file exists, and given its mode.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::harness::Machine;

fn file(root: &Path, unit: &str) -> PathBuf {
    root.join(".tay/state/files").join(unit).join(unit)
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_file_is_given_its_mode_and_one_with_a_secret_is_for_its_owner_alone() {
    let machine = Machine::new();
    let root = machine.stack("files");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(
        fs::read_to_string(file(&root, "plain")).unwrap(),
        "greeting: hello\n"
    );
    assert_eq!(mode(&file(&root, "plain")), 0o644);
    assert_eq!(mode(&file(&root, "secret-holding")), 0o600);
    assert_eq!(mode(&file(&root, "own-mode")), 0o640, "a mode written wins");
    // A check finds each as it was written.
    let status = machine.at(&root, "status", &[]);
    assert!(
        status.stdout.contains("plain           ready     written"),
        "{}",
        status.stdout
    );
}

#[test]
fn a_file_is_written_again_only_when_its_content_would_differ_and_moved_into_place() {
    let machine = Machine::new();
    let root = machine.stack("files");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let plain = file(&root, "plain");
    let first = fs::metadata(&plain).unwrap();
    let again = machine.at(&root, "up", &[]);
    assert_eq!(again.last_change("plain"), "ready as written before");
    let unchanged = fs::metadata(&plain).unwrap();
    assert_eq!(
        (unchanged.ino(), unchanged.mtime()),
        (first.ino(), first.mtime())
    );
    let changed = machine.at(&root, "up", &["--set", "settings.greeting=welcome"]);
    assert_eq!(changed.last_change("plain"), "ready written again");
    assert_eq!(fs::read_to_string(&plain).unwrap(), "greeting: welcome\n");
    // Written whole beside it and moved into place: a new file, and nothing
    // left beside it.
    assert_ne!(fs::metadata(&plain).unwrap().ino(), first.ino());
    let beside: Vec<_> = fs::read_dir(plain.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(beside, ["plain"]);
}

#[test]
fn a_file_with_overwrite_false_is_never_written_again_once_it_exists() {
    let machine = Machine::new();
    let root = machine.stack("files");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let once = file(&root, "written-once");
    assert_eq!(fs::read_to_string(&once).unwrap(), "greeting: hello\n");
    fs::write(&once, "rewritten by another program\n").unwrap();
    let up = machine.at(&root, "up", &["--set", "settings.greeting=welcome"]);
    assert_eq!(
        up.last_change("written-once"),
        "ready left as it is, since `overwrite` is false"
    );
    assert_eq!(
        fs::read_to_string(&once).unwrap(),
        "rewritten by another program\n"
    );
}

#[test]
fn a_mode_changed_by_hand_is_given_back_without_writing_the_file() {
    let machine = Machine::new();
    let root = machine.stack("files");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let plain = file(&root, "plain");
    let first = fs::metadata(&plain).unwrap().ino();
    fs::set_permissions(&plain, fs::Permissions::from_mode(0o666)).unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.last_change("plain"), "ready given its mode");
    assert_eq!(mode(&plain), 0o644);
    assert_eq!(fs::metadata(&plain).unwrap().ino(), first);
}
