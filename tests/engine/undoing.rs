//! A command with `when_removed: undo` keeps, once it has run, what its
//! `undo` needs: the command, where it runs and its environment, the
//! secrets with the secrets. At a `tay up` whose plan no longer has it, its
//! `undo` runs with what it last ran with, before anything else the plan no
//! longer has is stopped, and then it is forgotten; one that fails is said,
//! kept, and run again at the next `tay up`. `tay wipe --all` runs none.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::harness::Machine;

/// What `made` has once its `undo` has run.
const UNDONE: &str = "stopped it is not in the plan any more, and its `undo` has run";

/// The stack `undoing`, brought up, with `made` done.
fn brought_up(machine: &Machine) -> PathBuf {
    let root = machine.stack("undoing");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(root.join("made").exists());
    root
}

/// The token's value, as the environment at `root` keeps it.
fn token(root: &Path) -> String {
    let text = fs::read_to_string(root.join(".tay/state/secrets/token")).unwrap();
    let kept: serde_json::Value = serde_json::from_str(&text).unwrap();
    kept["value"].as_str().unwrap().to_string()
}

#[test]
fn a_command_that_leaves_the_plan_is_undone_with_what_it_ran_with_before_anything_else_stops() {
    let machine = Machine::new();
    let root = brought_up(&machine);
    let token = token(&root);
    // What its `undo` needs is kept, and its secret with the secrets, for
    // this account alone.
    let kept = fs::read_to_string(root.join(".tay/state/units/made/undo.json")).unwrap();
    assert!(!kept.contains(&token), "{kept}");
    let secrets = root.join(".tay/state/secrets/made.undo");
    assert!(fs::read_to_string(&secrets).unwrap().contains(&token));
    let mode = fs::metadata(&secrets).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    // It and `holder` leave the plan, as the setting it ran with changes.
    let up = machine.at(
        &root,
        "up",
        &["--set", "up=token", "--set", "settings.mode=two"],
    );
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let changes = up.changes();
    let line = |wanted: &str| {
        changes
            .iter()
            .position(|change| change == wanted)
            .unwrap_or_else(|| panic!("`{wanted}` is said:\n{}", up.stdout))
    };
    let undone = line(&format!("made {UNDONE}"));
    let stopped = line("holder stopped it is not in the plan any more, so it was stopped");
    assert!(undone < stopped, "{}", up.stdout);
    assert_eq!(
        fs::read_to_string(root.join("undone")).unwrap(),
        format!("one {token} running\n")
    );
    assert!(!root.join("made").exists());
    // Then it is forgotten.
    assert!(!root.join(".tay/state/units/made").exists());
    assert!(!secrets.exists());
    let again = machine.at(&root, "up", &[]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert!(!again.stdout.contains("made"), "{}", again.stdout);
}

#[test]
fn an_undo_that_fails_is_said_kept_and_run_again_at_the_next_up() {
    let machine = Machine::new();
    let root = brought_up(&machine);
    fs::write(root.join("refuse"), "").unwrap();
    let up = machine.at(&root, "up", &["--set", "up=token"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let failed = up.last_change("made");
    assert!(
        failed.starts_with("failed it is not in the plan any more, and its `undo` exited with 1"),
        "{failed}"
    );
    assert!(
        failed.ends_with(": the next `tay up` runs it again"),
        "{failed}"
    );
    assert!(root.join("made").exists());
    assert!(root.join(".tay/state/units/made/undo.json").exists());
    fs::remove_file(root.join("refuse")).unwrap();
    let again = machine.at(&root, "up", &[]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert_eq!(again.last_change("made"), UNDONE);
    assert!(!root.join("made").exists());
}

#[test]
fn wipe_all_runs_no_undo() {
    let machine = Machine::new();
    let root = brought_up(&machine);
    let wipe = machine.at(&root, "wipe", &["--all", "--yes"]);
    assert_eq!(wipe.code, 0, "{}{}", wipe.stdout, wipe.stderr);
    assert!(root.join("made").exists());
    assert!(!root.join("undone").exists());
    let up = machine.at(&root, "up", &["--set", "up=token"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(!up.stdout.contains("made"), "{}", up.stdout);
    assert!(root.join("made").exists());
}

#[test]
fn a_command_done_again_with_when_removed_keep_is_not_undone() {
    let machine = Machine::new();
    let root = brought_up(&machine);
    let definitions = fs::read_to_string(root.join("tay.yaml")).unwrap();
    fs::write(
        root.join("tay.yaml"),
        definitions.replace("when_removed: undo", "when_removed: keep"),
    )
    .unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(!root.join(".tay/state/units/made/undo.json").exists());
    let up = machine.at(&root, "up", &["--set", "up=token"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(root.join("made").exists());
    assert!(!root.join("undone").exists());
}
