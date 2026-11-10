//! `tay wipe UNIT... --yes` and `tay wipe --all --yes`: what a unit keeps
//! is removed, each running unit that needs it stopped first, and on an
//! environment meant to be up everything is brought up again, so that
//! commands check and run again where their work is gone; a command whose
//! `undo` needs a unit that is not up is refused; `--all` takes the
//! environment down and removes everything it keeps; and without `--yes`
//! nothing changes and the command says what it would remove.

use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::harness::{Machine, child, eventually, kept_ports};

fn lines(path: &Path) -> usize {
    fs::read_to_string(path).unwrap_or_default().lines().count()
}

#[test]
fn wiping_a_command_runs_its_undo_and_brings_up_again_what_needed_it() {
    let machine = Machine::new();
    let root = machine.stack("wipe");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let state = root.join(".tay/state");
    // A process is ready once it has started, which may be before it has
    // written anything.
    let starts = state.join("app-starts");
    assert!(eventually(Duration::from_secs(10), || lines(&starts) == 1));
    let (store, app) = (child(&root, "store"), child(&root, "app"));
    // Without --yes it says what it would remove, and changes nothing.
    let asked = machine.at(&root, "wipe", &["schema"]);
    assert_eq!(asked.code, 0, "{}{}", asked.stdout, asked.stderr);
    assert_eq!(
        asked.stdout,
        "tay wipe: `schema` keeps what its `undo` undoes; the values it produced; its record\n\
         tay wipe: nothing was removed; `--yes` removes it.\n"
    );
    assert!(state.join("units/schema/values.json").exists());
    assert_eq!(child(&root, "app"), app);
    let wipe = machine.at(&root, "wipe", &["schema", "--yes"]);
    assert_eq!(wipe.code, 0, "{}{}", wipe.stdout, wipe.stderr);
    assert!(
        wipe.stdout.contains(
            "tay wipe: removed what `schema` kept: what its `undo` undoes; the values it produced; its record\n"
        ),
        "{}",
        wipe.stdout
    );
    // What needed it was stopped and started again, and the command, its
    // work undone, found itself not done and ran again.
    assert_eq!(wipe.changes()[0], "app stopped stopped by `tay wipe`");
    assert_eq!(lines(&state.join("schema-runs")), 2);
    assert!(
        eventually(Duration::from_secs(10), || lines(&starts) == 2),
        "{}",
        crate::harness::log(&root, "app")
    );
    assert_ne!(child(&root, "app"), app);
    // What it needs was left as it runs.
    assert_eq!(child(&root, "store"), store);
    assert!(
        wipe.stdout
            .ends_with("tay wipe: everything asked for is ready.\n"),
        "{}",
        wipe.stdout
    );
}

#[test]
fn wiping_what_is_made_or_a_process_on_an_environment_down_removes_it_and_brings_nothing_up() {
    let machine = Machine::new();
    let root = machine.stack("wipe");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(machine.at(&root, "down", &[]).code, 0);
    let state = root.join(".tay/state");
    let number = kept_ports(&root)["app-port"].as_u64().unwrap();
    assert!(state.join("units/store/record.json").exists());
    let wipe = machine.at(
        &root,
        "wipe",
        &["app-port", "token", "conf", "store", "--yes"],
    );
    assert_eq!(wipe.code, 0, "{}{}", wipe.stdout, wipe.stderr);
    // A process keeps nothing beyond its record.
    assert!(!state.join("units/store").exists());
    assert!(!kept_ports(&root).contains_key("app-port"));
    let listed = &machine.list()["environments"][0]["ports"];
    assert!(
        !listed
            .as_array()
            .unwrap()
            .iter()
            .any(|port| port.as_u64() == Some(number)),
        "{listed}"
    );
    assert!(!state.join("secrets/token").exists());
    assert!(!state.join("files/conf/conf").exists());
    assert!(
        wipe.stdout.ends_with(
            "tay wipe: done; the environment is down, so nothing was brought up again.\n"
        ),
        "{}",
        wipe.stdout
    );
    assert!(
        machine
            .processes()
            .iter()
            .all(|pid| { crate::harness::command_of(*pid).contains("tay daemon") })
    );
    // The next `tay up` makes them afresh.
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert!(state.join("secrets/token").exists());
    assert!(state.join("files/conf/conf").exists());
}

#[test]
fn a_command_whose_undo_needs_a_unit_that_is_not_up_is_refused() {
    let machine = Machine::new();
    let root = machine.stack("wipe");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let refused = "tay wipe: wiping `schema` runs what undoes its work, which needs `store`, and `store` is not up\n";
    // Wiped with it, what it needs would be stopped.
    let both = machine.at(&root, "wipe", &["store", "schema", "--yes"]);
    assert_eq!((both.code, both.stderr.as_str()), (4, refused));
    assert_eq!(machine.at(&root, "down", &[]).code, 0);
    let down = machine.at(&root, "wipe", &["schema", "--yes"]);
    assert_eq!((down.code, down.stderr.as_str()), (4, refused));
    // Nothing was undone.
    assert!(root.join(".tay/state/schema-made").exists());
}

#[test]
fn a_unit_not_in_the_plan_or_a_group_does_not_fit() {
    let machine = Machine::new();
    let root = machine.stack("wipe");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let nobody = machine.at(&root, "wipe", &["nobody", "--yes"]);
    assert_eq!(
        (nobody.code, nobody.stderr.as_str()),
        (
            2,
            "tay wipe: there is no unit `nobody` in this environment's plan\n"
        )
    );
    let neither = machine.at(&root, "wipe", &[]);
    assert_eq!(neither.code, 2);
    let both = machine.at(&root, "wipe", &["app", "--all"]);
    assert_eq!(both.code, 2);
}

#[test]
fn wipe_all_takes_the_environment_down_and_removes_everything_it_keeps() {
    let machine = Machine::new();
    let root = machine.stack("wipe");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    fs::write(root.join(".tay/choices.yaml"), "label: kept\n").unwrap();
    let asked = machine.at(&root, "wipe", &["--all"]);
    assert_eq!(
        asked.stdout,
        "tay wipe: --all takes the environment down and removes `.tay/` but for `choices.yaml`; its place on this machine's list, with the ports it kept\n\
         tay wipe: nothing was removed; `--yes` removes it.\n"
    );
    assert!(machine.daemon(&root).is_some());
    let wipe = machine.at(&root, "wipe", &["--all", "--yes"]);
    assert_eq!(wipe.code, 0, "{}{}", wipe.stdout, wipe.stderr);
    let mut left: Vec<String> = fs::read_dir(root.join(".tay"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["choices.yaml", "daemon.lock"]);
    assert_eq!(
        fs::read_to_string(root.join(".tay/choices.yaml")).unwrap(),
        "label: kept\n"
    );
    assert!(
        machine.list()["environments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(machine.processes().is_empty(), "{:?}", machine.processes());
    // It comes up again from nothing.
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(lines(&root.join(".tay/state/schema-runs")), 1);
}
