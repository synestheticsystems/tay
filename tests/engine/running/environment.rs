//! What a program on the host runs with: the variables its definition
//! gives, and those `inherit` names from the environment tay runs in, the
//! environment of the command that started the daemon, which are `PATH`,
//! `HOME`, `USER`, `LANG` and `TMPDIR` when it does not say; nothing else.
//! It runs in its `workdir`, or the stack's root.

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

#[test]
fn a_command_on_the_host_inherits_in_its_check_and_its_run_what_inherit_names_and_nothing_else() {
    let machine = Machine::new();
    let root = machine.stack("services");
    let mut up = machine.command(
        &root,
        &["up", "--root", root.to_str().unwrap(), "inheriting"],
    );
    // The command that starts the daemon has a variable the unit names, and
    // one it does not.
    up.env("AGENT_SOCKET", "/run/agent.sock")
        .env("UNNAMED", "not passed on");
    let up = machine.run(up);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let said = log(&root, "inheriting");
    let (check, run) = said
        .split_once(" | checked\n")
        .unwrap_or_else(|| panic!("its check said it was not done:\n{said}"));
    for (step, said) in [("check", check), ("run", run)] {
        assert_eq!(
            variables(said),
            ["AGENT_SOCKET", "MODE", "PATH", "TAY_VALUES"],
            "{step}: {said}"
        );
        assert!(
            said.contains(" | variable AGENT_SOCKET=/run/agent.sock\n"),
            "{step}: {said}"
        );
    }
}
