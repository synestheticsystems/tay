//! A remembered choice that no longer fits the stack, as when a change of
//! branch takes away a setting, a unit or a profile it names, is said to be
//! remembered, with the `tay unset` that forgets it; `tay unset` forgets it
//! though the stack no longer plans, whether or not a daemon runs; and
//! `tay unset units` forgets the units named on an earlier `tay up`.

use std::fs;
use std::path::Path;

use serde_json::json;

use super::greeting;
use crate::harness::Machine;

/// The stack `remembered` at `root` as another branch has it: with no
/// setting `name`, no unit `token-file` and no profile `loud`.
fn change_branch(root: &Path) {
    let stack = "format: 1
stack: remembered
default_profile: quiet
settings:
  greeting: { default: hello }
up: [greeting]
units:
  greeting:
    uses: file
    with: { content: \"${settings.greeting}\\n\" }
";
    fs::write(root.join("tay.yaml"), stack).unwrap();
    fs::remove_file(root.join("stack/profiles/loud.yaml")).unwrap();
}

#[test]
fn a_remembered_choice_the_stack_no_longer_has_is_said_to_be_remembered_with_what_forgets_it() {
    let machine = Machine::new();
    let root = machine.stack("remembered");
    let up = machine.at(
        &root,
        "up",
        &[
            "token-file",
            "--profile",
            "loud",
            "--set",
            "settings.name=there",
        ],
    );
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(machine.at(&root, "down", &[]).code, 0);
    change_branch(&root);
    // Every command that plans says so, none of it typed on that command.
    let remembered = [
        "remembered `--profile loud`: there is no profile `loud`; the profiles are `quiet`; `tay unset profile` forgets it",
        "remembered `--set settings.name`: no setting `name` is declared in tay.yaml, so it cannot be set; `tay unset settings.name` forgets it",
        "remembered `tay up token-file`: there is no unit `token-file`; `tay unset units` forgets the units named then",
    ];
    for command in ["plan", "status", "up"] {
        let run = machine.at(&root, command, &[]);
        assert_eq!(run.code, 2, "tay {command}: {}{}", run.stdout, run.stderr);
        let closing = format!("tay {command}: ");
        let errors: Vec<&str> = (run.stderr.lines())
            .filter(|line| !line.starts_with(&closing))
            .collect();
        assert_eq!(errors, remembered, "tay {command}");
    }
    // What a command gives is its own, and replaces what is remembered for
    // its place.
    let run = machine.at(
        &root,
        "plan",
        &["--profile", "quiet", "--set", "settings.name=again"],
    );
    assert_eq!(
        run.errors(),
        [
            "--set settings.name: no setting `name` is declared in tay.yaml, so it cannot be set",
            remembered[2],
        ]
    );
    // In JSON, a remembered place is an argument, written as remembered,
    // and the message ends with what forgets it.
    let json = machine.at(&root, "plan", &["--json"]).json();
    assert_eq!(
        json["errors"][1],
        json!({
            "at": { "argument": "remembered `--set settings.name`" },
            "message": "no setting `name` is declared in tay.yaml, so it cannot be set; `tay unset settings.name` forgets it",
            "related": [],
        })
    );
}

#[test]
fn tay_unset_units_forgets_the_units_named_on_an_earlier_up_so_that_what_up_lists_comes_up() {
    let machine = Machine::new();
    let root = machine.stack("remembered");
    let up = machine.at(&root, "up", &["token-file"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let unset = machine.at(&root, "unset", &["units"]);
    assert_eq!(unset.code, 0, "{}{}", unset.stdout, unset.stderr);
    assert_eq!(unset.stdout, "nothing is remembered\n");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(greeting(&root), "hello, world\n");
    let again = machine.at(&root, "unset", &["units"]);
    assert_eq!(
        (again.code, again.stderr.as_str()),
        (
            2,
            "tay unset: nothing is remembered for `units`, so nothing was forgotten\n"
        )
    );
    // `units` is a place for `tay unset` alone.
    for command in ["set", "plan"] {
        let args: &[&str] = match command {
            "set" => &["units=token-file"],
            _ => &["--set", "units=token-file"],
        };
        let run = machine.at(&root, command, args);
        assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
        assert!(
            run.stderr.starts_with("--set units: `--set` sets `up`, `up_add`, `up_remove`, `run.default`, `run.units.UNIT`, `settings.NAME` or `label`\n"),
            "{}",
            run.stderr
        );
    }
}

#[test]
fn tay_unset_forgets_a_choice_the_stack_no_longer_plans_with_whether_or_not_a_daemon_runs() {
    let machine = Machine::new();
    for root in [
        machine.stack_as("remembered", "daemon-running"),
        machine.stack_as("remembered", "no-daemon"),
    ] {
        let up = machine.at(&root, "up", &["--set", "settings.name=there"]);
        assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
        let daemon = match root.ends_with("no-daemon") {
            true => {
                assert_eq!(machine.at(&root, "down", &[]).code, 0);
                None
            }
            false => Some(machine.daemon_pid(&root).expect("a daemon answers")),
        };
        change_branch(&root);
        assert_eq!(machine.at(&root, "plan", &[]).code, 2);
        let unset = machine.at(&root, "unset", &["settings.name"]);
        assert_eq!(unset.code, 0, "{}{}", unset.stdout, unset.stderr);
        assert_eq!(unset.stdout, "nothing is remembered\n");
        // The daemon that runs made the change; with none, none was
        // started for it.
        assert_eq!(machine.daemon_pid(&root), daemon, "{}", root.display());
        if daemon.is_some() {
            let log = fs::read_to_string(root.join(".tay/logs/tay/daemon.log")).unwrap();
            assert!(log.contains("change: tay unset settings.name\n"), "{log}");
        }
        let plan = machine.at(&root, "plan", &[]);
        assert_eq!(plan.code, 0, "{}{}", plan.stdout, plan.stderr);
    }
}
