//! What every command shares: `--root`, `--json` in a versioned schema,
//! reading nothing from a terminal, and the help.

use std::process::Stdio;
use std::time::Duration;

use crate::harness::Machine;

#[test]
fn no_command_reads_from_the_terminal() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    for args in [&["up"][..], &["status"], &["ls"], &["down"]] {
        // Standard input is held open and never written: a command that
        // read it would wait for ever.
        let mut command = machine.command(&root, args);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        let _held = child.stdin.take();
        let ended = crate::harness::eventually(Duration::from_secs(20), || {
            child.try_wait().unwrap().is_some()
        });
        assert!(ended, "`tay {}` waited on its input", args[0]);
    }
}

#[test]
fn every_command_takes_root_and_finds_the_stack_above_the_current_folder() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    let elsewhere = machine.path("home");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    for command in ["status", "ls", "down"] {
        let run = machine.tay(&elsewhere, &[command, "--root", root.to_str().unwrap()]);
        assert_eq!(run.code, 0, "{command}: {}", run.stderr);
    }
    let up = machine.tay(&root.join(".tay"), &["up"]);
    assert_eq!(up.code, 0, "{}", up.stderr);
    let lost = machine.tay(&elsewhere, &["status"]);
    assert_eq!(lost.code, 2);
    assert!(
        lost.stderr
            .starts_with("tay status: there is no tay.yaml in ")
    );
}

#[test]
fn down_with_json_prints_its_events_in_schema_1() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let down = machine.at(&root, "down", &["--json"]);
    assert_eq!(down.code, 0);
    let last: serde_json::Value =
        serde_json::from_str(down.stdout.lines().last().unwrap()).unwrap();
    assert_eq!(last["schema"], 1);
    assert_eq!(last["event"], "down");
    assert_eq!(last["left"], serde_json::json!([]));
}

#[test]
fn the_help_says_the_environment_remembers_what_up_was_asked() {
    let machine = Machine::new();
    let help = machine.tay(&machine.path("home"), &["--help"]);
    assert!(
        help.stdout
            .contains("The environment remembers the profile, each --set and the units named")
    );
    for usage in [
        "tay up [UNIT...] [--root FOLDER] [--profile NAME] [--set PLACE=VALUE]... [--json]",
        "tay down [--root FOLDER] [--json]",
        "tay status [UNIT] [--root FOLDER] [--json]",
        "tay logs [UNIT...] [--root FOLDER] [--since TIME] [--follow]",
        "tay ls [--root FOLDER] [--json]",
    ] {
        assert!(help.stdout.contains(usage), "{usage}");
    }
}

#[test]
fn an_option_a_command_does_not_take_is_an_error() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    let down = machine.at(&root, "down", &["--profile", "dev"]);
    assert_eq!(down.code, 2);
    assert_eq!(
        down.stderr,
        "tay down: `--profile` is not an option of `tay down`\nRun `tay --help` for how to use it.\n"
    );
    let status = machine.at(&root, "status", &["one", "two"]);
    assert_eq!(status.code, 2);
    assert!(
        status
            .stderr
            .starts_with("tay status: `tay status` names at most one unit; `two` is a second")
    );
}
