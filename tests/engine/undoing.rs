//! A command with `when_removed: undo` keeps, once it has run, what its
//! `undo` needs: the command, where it runs and its environment, the
//! secrets with the secrets, and the units it needed. At a `tay up` whose
//! plan no longer has it, its `undo` runs with what it last ran with, once
//! the units it needed that the plan has are ready, and before anything
//! else the plan no longer has is stopped; then it is forgotten. One that
//! fails, or cannot run because a unit it needed is neither up nor in the
//! plan, is said, kept, and tried again at the next `tay up`.
//! `tay wipe --all` runs none.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::harness::Machine;

/// What `made` has once its `undo` has run.
const UNDONE: &str = "stopped it is not in the plan any more, and its `undo` has run";

/// What `made` has when its `undo` cannot run, `holder` having left the
/// plan and stopped.
const CANNOT: &str = "failed it is not in the plan any more, and its `undo` cannot run: it needed `holder`, which is neither up nor in the plan any more; the next `tay up` tries it again";

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
fn an_undo_that_fails_is_said_kept_and_tried_again_at_the_next_up() {
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
        failed.ends_with("; the next `tay up` tries it again"),
        "{failed}"
    );
    assert!(root.join("made").exists());
    assert!(root.join(".tay/state/units/made/undo.json").exists());
    // `holder` was stopped once the `undo` had come to its end, so the
    // next `tay up` finds it neither up nor in the plan.
    fs::remove_file(root.join("refuse")).unwrap();
    let again = machine.at(&root, "up", &[]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert_eq!(again.last_change("made"), CANNOT);
    assert!(root.join("made").exists());
    let holder_back = machine.at(&root, "up", &["--set", "up=token,holder"]);
    assert_eq!(
        holder_back.code, 0,
        "{}{}",
        holder_back.stdout, holder_back.stderr
    );
    assert_eq!(holder_back.last_change("made"), UNDONE);
    assert!(!root.join("made").exists());
}

#[test]
fn an_undo_on_an_environment_that_was_down_waits_for_the_units_it_needed() {
    let machine = Machine::new();
    let root = brought_up(&machine);
    let token = token(&root);
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    // `made` leaves the plan, and `holder`, which it needed, stays.
    let up = machine.at(&root, "up", &["--set", "up=token,holder"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let changes = up.changes();
    let line = |wanted: &str| {
        changes
            .iter()
            .position(|change| change.starts_with(wanted))
            .unwrap_or_else(|| panic!("`{wanted}` is said:\n{}", up.stdout))
    };
    let waits =
        line("made waiting it is not in the plan any more, and its `undo` waits for `holder`");
    let ready = line("holder ready");
    let undone = line(&format!("made {UNDONE}"));
    assert!(waits < ready && ready < undone, "{}", up.stdout);
    assert_eq!(
        fs::read_to_string(root.join("undone")).unwrap(),
        format!("one {token} running\n")
    );
    assert!(!root.join("made").exists());
    assert!(!root.join(".tay/state/units/made").exists());
}

#[test]
fn an_undo_that_needed_a_unit_neither_up_nor_in_the_plan_cannot_run_and_is_kept() {
    let machine = Machine::new();
    let root = brought_up(&machine);
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    let up = machine.at(&root, "up", &["--set", "up=token"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(up.last_change("made"), CANNOT);
    assert!(root.join("made").exists());
    assert!(!root.join("undone").exists());
    assert!(root.join(".tay/state/units/made/undo.json").exists());
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
