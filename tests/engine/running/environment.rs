//! What a program on the host runs with: the variables its definition
//! gives, and those `inherit` names from the environment tay runs in, which
//! are `PATH`, `HOME`, `USER`, `LANG` and `TMPDIR` when it does not say;
//! nothing else. It runs in its `workdir`, or the stack's root.

use std::time::Duration;

use crate::harness::{Machine, eventually, log};

/// The variables a reporting unit said it was given, by name.
fn variables(log: &str) -> Vec<String> {
    log.lines()
        .filter_map(|line| {
            line.split_once(" | variable ")
                .map(|(_, variable)| variable)
        })
        .map(|variable| {
            variable
                .split_once('=')
                .map_or(variable, |(name, _)| name)
                .to_string()
        })
        .filter(|name| !["PWD", "SHLVL", "_"].contains(&name.as_str()))
        .collect()
}

#[test]
fn a_process_runs_with_only_what_its_definition_gives_and_inherits_in_its_working_folder() {
    let machine = Machine::new();
    let root = machine.stack("services");
    assert_eq!(machine.at(&root, "up", &["reporter"]).code, 0);
    assert!(eventually(Duration::from_secs(20), || log(
        &root, "reporter"
    )
    .contains("variable PATH=")));
    let said = log(&root, "reporter");
    assert_eq!(variables(&said), ["MODE", "PATH"], "{said}");
    assert!(said.contains(" | variable MODE=fast"), "{said}");
    assert!(
        said.contains(&format!(" | in {}", root.join(".tay/state").display())),
        "{said}"
    );
}

#[test]
fn a_command_inherits_what_a_process_does_by_default_and_is_told_where_to_write_its_values() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let up = machine.at(&root, "up", &["reported"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let said = log(&root, "reported");
    // The test runs tay with only these of the five, and the XDG folders,
    // which are not passed on.
    assert_eq!(variables(&said), ["HOME", "PATH", "TAY_VALUES"], "{said}");
    assert!(
        said.contains(&format!(
            " | variable TAY_VALUES={}",
            root.join(".tay/state/units/reported/produced.json")
                .display()
        )),
        "{said}"
    );
    assert!(
        said.contains(&format!(" | in {}", root.display())),
        "{said}"
    );
}
