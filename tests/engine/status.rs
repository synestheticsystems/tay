//! `tay status [UNIT]` and `tay ls`: each unit's state from a check made
//! now, and the environments on this machine.

use std::fs;

use crate::harness::{Machine, alive, eventually, kept_ports};

#[test]
fn status_is_from_a_check_made_now() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let status = machine.at(&root, "status", &[]);
    assert_eq!(status.code, 0);
    assert!(
        status.stdout.contains("config       ready     written"),
        "{}",
        status.stdout
    );
    // A file that has gone is reported so, though the daemon made it.
    let config = root.join(".tay/state/files/config/config");
    fs::remove_file(&config).unwrap();
    let status = machine.at(&root, "status", &[]);
    assert_eq!(status.code, 1, "a unit has failed");
    assert!(
        status.stdout.contains(&format!(
            "config       failed    `{}` does not exist",
            config.display()
        )),
        "{}",
        status.stdout
    );
    // The next `tay up` makes it again.
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert!(config.exists());
}

#[test]
fn status_of_one_unit_and_of_one_the_plan_does_not_have() {
    let machine = Machine::new();
    let root = machine.stack("made-and-run");
    machine.at(&root, "up", &[]);
    machine.settled(&root);
    let site = machine.at(&root, "status", &["site"]);
    assert_eq!(site.code, 1);
    assert_eq!(
        site.stdout,
        format!(
            "{}: up\n  site  failed    tay cannot run a `process` unit yet\n",
            root.display()
        )
    );
    let port = machine.at(&root, "status", &["site-port", "--json"]).json();
    assert_eq!(port["schema"], 1);
    assert_eq!(port["units"].as_array().unwrap().len(), 1);
    assert_eq!(port["units"][0]["state"], "ready");
    assert!(port["units"][0]["since"].is_u64());
    let nobody = machine.at(&root, "status", &["nobody"]);
    assert_eq!(nobody.code, 2);
    assert_eq!(
        nobody.stderr,
        "tay status: there is no unit `nobody` in this environment's plan\n"
    );
}

#[test]
fn an_environment_never_brought_up_has_every_unit_stopped_and_no_daemon() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    let status = machine.at(&root, "status", &["--json"]);
    assert_eq!(status.code, 0, "{}", status.stderr);
    let status = status.json();
    assert_eq!(status["environment"]["standing"], "never brought up");
    assert!(status["environment"]["id"].is_null());
    for unit in status["units"].as_array().unwrap() {
        assert_eq!(unit["state"], "stopped");
        assert!(unit["since"].is_null());
    }
    assert!(!root.join(".tay").exists(), "status writes nothing");
}

#[test]
fn ls_lists_each_environment_with_its_folder_and_ports_and_marks_this_one() {
    let machine = Machine::new();
    let first = machine.stack_as("waiting", "waiting");
    let second = machine.stack_as("waiting", "waiting-fix-login");
    assert_eq!(machine.at(&first, "up", &[]).code, 0);
    assert_eq!(machine.at(&second, "up", &[]).code, 0);
    assert_eq!(machine.at(&second, "down", &[]).code, 0);
    let ls = machine.tay(&first, &["ls"]);
    assert_eq!(ls.code, 0);
    let line = |root: &std::path::Path| {
        ls.stdout
            .lines()
            .find(|line| line.contains(root.to_str().unwrap()))
            .unwrap()
            .to_string()
    };
    let port = |root: &std::path::Path| kept_ports(root)["config-port"].clone();
    let first_line = line(&first);
    assert!(first_line.starts_with("* waiting "), "{first_line}");
    assert!(first_line.contains(&machine.id(&first)));
    assert!(first_line.contains(", daemon "), "{first_line}");
    assert!(
        first_line.ends_with(&format!("ports {}", port(&first))),
        "{first_line}"
    );
    let second_line = line(&second);
    assert!(
        second_line.starts_with("  waiting-fix-login "),
        "{second_line}"
    );
    assert!(
        second_line.contains(&format!("down, ports {}", port(&second))),
        "{second_line}"
    );
    let json = machine.tay(&first, &["ls", "--json"]).json();
    assert_eq!(json["schema"], 1);
    assert_eq!(json["environments"].as_array().unwrap().len(), 2);
}

#[test]
fn ls_shows_the_folder_so_two_environments_of_one_name_can_be_told_apart() {
    let machine = Machine::new();
    let one = machine.stack_as("waiting", "one/waiting");
    let two = machine.stack_as("waiting", "two/waiting");
    assert_eq!(machine.at(&one, "up", &[]).code, 0);
    assert_eq!(machine.at(&two, "up", &[]).code, 0);
    let ls = machine.tay(&two, &["ls"]);
    let lines: Vec<&str> = ls.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", ls.stdout);
    for (root, mark) in [(&one, ' '), (&two, '*')] {
        let line = lines
            .iter()
            .find(|line| line.contains(root.to_str().unwrap()))
            .unwrap();
        assert!(line.starts_with(&format!("{mark} waiting  ")), "{line}");
    }
}

#[test]
fn an_environment_leaves_the_list_once_nothing_of_it_is_left() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let pid = machine.daemon_pid(&root).unwrap();
    fs::remove_dir_all(&root).unwrap();
    // Its daemon stops what it started and exits.
    assert!(eventually(std::time::Duration::from_secs(5), || !alive(
        pid
    )));
    let ls = machine.tay(&machine.path("home"), &["ls"]);
    assert_eq!(ls.stdout, "tay knows no environment on this machine.\n");
    assert_eq!(machine.list()["environments"], serde_json::json!([]));
}

#[test]
fn a_missing_folder_alone_does_not_take_an_environment_off_the_list() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(machine.at(&root, "down", &[]).code, 0);
    // Something of it still answers on its socket.
    let socket = machine.socket(&root);
    let answering = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    fs::remove_dir_all(&root).unwrap();
    let ls = machine.tay(&machine.path("home"), &["ls"]);
    assert!(ls.stdout.contains("its folder is missing"), "{}", ls.stdout);
    drop(answering);
    fs::remove_file(&socket).unwrap();
    let ls = machine.tay(&machine.path("home"), &["ls"]);
    assert_eq!(ls.stdout, "tay knows no environment on this machine.\n");
}
