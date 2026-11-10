//! Secrets and containers: a secret reaches a container, and a command run
//! in one, through its environment, which Docker shows to whoever may ask
//! it, and nowhere else: not in any output, any log, any label, any record,
//! nor in the argument list of anything tay starts or creates.

use std::process::Command;
use std::time::Duration;

use super::container;
use crate::harness::{Machine, eventually, log};

#[test]
fn no_secret_appears_in_any_output_log_label_or_argument_list() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let mut printed = String::new();
    for args in [
        &["up", "guarded", "stamped"][..],
        &["status"],
        &["status", "--json"],
    ] {
        let run = machine.at(&root, args[0], &args[1..]);
        printed.push_str(&run.stdout);
        printed.push_str(&run.stderr);
    }
    let text = std::fs::read_to_string(root.join(".tay/state/secrets/password")).unwrap();
    let kept: serde_json::Value = serde_json::from_str(&text).unwrap();
    let secret = kept["value"].as_str().unwrap().to_string();
    // It reached the container, through its environment.
    assert!(eventually(Duration::from_secs(20), || {
        log(&root, "guarded").contains("| has a password")
    }));
    let guarded = container(&machine, &root, "guarded").unwrap();
    let env = guarded["Config"]["Env"].to_string();
    assert!(env.contains(&format!("PASSWORD={secret}")));
    // And nowhere else Docker shows of it.
    let mut shown = guarded["Config"]["Labels"].to_string();
    for field in ["Cmd", "Entrypoint"] {
        shown.push_str(&guarded["Config"][field].to_string());
    }
    shown.push_str(&guarded["Args"].to_string());
    // What it was started from is kept by its secret's name.
    let started =
        std::fs::read_to_string(root.join(".tay/state/units/guarded/started-from.json")).unwrap();
    assert!(started.contains("secret_env.PASSWORD"), "{started}");
    printed.push_str(&started);
    for unit in ["guarded", "stamped"] {
        printed.push_str(&log(&root, unit));
        printed.push_str(&machine.at(&root, "logs", &[unit]).stdout);
    }
    let ps = Command::new("ps")
        .args(["-ax", "-o", "command="])
        .output()
        .unwrap();
    printed.push_str(&String::from_utf8_lossy(&ps.stdout));
    let down = machine.at(&root, "down", &["--json"]);
    printed.push_str(&down.stdout);
    printed.push_str(&std::fs::read_to_string(root.join(".tay/logs/tay/daemon.log")).unwrap());
    assert!(
        !shown.contains(&secret),
        "a secret is in what Docker shows: {shown}"
    );
    assert!(!printed.contains(&secret), "a secret was shown");
    // The command's container of its own is gone with its environment.
    assert!(container(&machine, &root, "stamped").is_none());
}

#[test]
fn a_container_whose_secret_changed_is_replaced_and_said_to_be_by_the_secrets_name() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    assert_eq!(machine.at(&root, "up", &["guarded"]).code, 0);
    let first = container(&machine, &root, "guarded").unwrap()["Id"].clone();
    // A secret that is no longer kept is generated again, another.
    std::fs::remove_file(root.join(".tay/state/secrets/password")).unwrap();
    let up = machine.at(&root, "up", &["guarded"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(
        up.changes().contains(
            &"guarded starting `secret_env.PASSWORD` changed, so it is started again".to_string()
        ),
        "{}",
        up.stdout
    );
    assert_ne!(container(&machine, &root, "guarded").unwrap()["Id"], first);
}
