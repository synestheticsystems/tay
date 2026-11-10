//! What identifies an environment and what is kept on disk: its id, its
//! `.tay/` folder, the machine's list of environments with the folder and
//! ports of each, and the folder a stack's environments share; a moved
//! folder keeps its id and a copied one is given its own.

use std::fs;
use std::path::Path;

use crate::harness::{Machine, copy, kept_ports};

fn listed<'l>(list: &'l serde_json::Value, root: &Path) -> Vec<&'l serde_json::Value> {
    list["environments"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|listed| listed["folder"] == root.to_str().unwrap())
        .collect()
}

#[test]
fn the_id_is_made_the_first_time_the_environment_is_brought_up_and_kept() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert!(!root.join(".tay").exists());
    assert_eq!(machine.at(&root, "plan", &[]).code, 0);
    assert!(!root.join(".tay").exists(), "planning makes no id");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let id = machine.id(&root);
    assert_eq!(id.len(), 12);
    assert!(
        id.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(machine.id(&root), id);
}

#[test]
fn the_environments_folder_is_laid_out_as_the_document_says() {
    let machine = Machine::new();
    let root = machine.stack("notes");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let tay = root.join(".tay");
    for path in [
        "id",
        "daemon.lock",
        "state/intent",
        "state/ports.json",
        "state/secrets/notes-key",
        "state/files/config/notes.conf",
        "logs/tay/daemon.log",
    ] {
        assert!(tay.join(path).exists(), "{path}");
    }
    assert_eq!(kept_ports(&root)["notes-port"], 24601);
}

#[test]
fn the_machines_list_has_each_environments_folder_and_ports() {
    let machine = Machine::new();
    let first = machine.stack_as("waiting", "waiting");
    let second = machine.stack_as("waiting", "waiting-fix-login");
    assert_eq!(machine.at(&first, "up", &[]).code, 0);
    assert_eq!(machine.at(&second, "up", &[]).code, 0);
    let list = machine.list();
    for root in [&first, &second] {
        let [listed] = listed(&list, root)[..] else {
            panic!("one entry for {}: {list}", root.display());
        };
        assert_eq!(listed["id"], machine.id(root).as_str());
        assert_eq!(
            listed["ports"],
            serde_json::json!([kept_ports(root)["config-port"]])
        );
    }
    // The folder shared by one person's environments of a stack.
    assert!(machine.path("data/tay/waiting").is_dir());
}

#[test]
fn a_moved_folder_keeps_its_id_and_a_copied_one_is_given_its_own() {
    let machine = Machine::new();
    let root = machine.stack_as("waiting", "waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    assert_eq!(machine.at(&root, "down", &[]).code, 0);
    let id = machine.id(&root);
    let port = kept_ports(&root)["config-port"].clone();
    let moved = machine.path("stacks/waiting-moved");
    fs::rename(&root, &moved).unwrap();
    let moved = moved.canonicalize().unwrap();
    machine.adopt(&moved);
    assert_eq!(machine.at(&moved, "up", &[]).code, 0);
    assert_eq!(machine.id(&moved), id);
    assert_eq!(
        kept_ports(&moved)["config-port"],
        port,
        "it keeps its ports"
    );
    let list = machine.list();
    let [moved_listed] = listed(&list, &moved)[..] else {
        panic!("one entry for the moved folder: {list}");
    };
    // The machine's list lists them again, so no other environment is
    // given them.
    assert_eq!(moved_listed["ports"], serde_json::json!([port]));
    assert!(listed(&list, &root).is_empty());
    // A copy, made while the original is up, is noticed and given its own
    // id; the ports it was copied with are its original's, which it does
    // not keep.
    let copied = machine.path("stacks/waiting-copy");
    copy(&moved, &copied);
    let copied = copied.canonicalize().unwrap();
    machine.adopt(&copied);
    assert_eq!(machine.id(&copied), id);
    let status = machine.at(&copied, "status", &[]);
    assert!(
        status.stdout.contains("this folder is a copy of"),
        "{}",
        status.stdout
    );
    assert_eq!(machine.at(&copied, "up", &[]).code, 0);
    assert_ne!(machine.id(&copied), id);
    assert_ne!(kept_ports(&copied)["config-port"], port);
    let list = machine.list();
    assert_eq!(listed(&list, &moved)[0]["id"], id.as_str());
    assert_eq!(
        listed(&list, &copied)[0]["id"],
        machine.id(&copied).as_str()
    );
}

#[test]
fn without_the_xdg_variables_the_documents_folders_are_used() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    let temporary = machine.path("tmp");
    fs::create_dir(&temporary).unwrap();
    let tay = |args: &[&str]| {
        let mut command = machine.command(&root, args);
        command
            .env_remove("XDG_STATE_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("XDG_RUNTIME_DIR")
            .env("TMPDIR", &temporary);
        machine.run(command)
    };
    let up = tay(&["up"]);
    assert_eq!(up.code, 0, "{}", up.stderr);
    let home = machine.path("home");
    assert!(home.join(".local/state/tay/environments.json").is_file());
    assert!(home.join(".local/share/tay/waiting").is_dir());
    let uid = rustix::process::getuid().as_raw();
    let socket = temporary
        .join(format!("tay-{uid}"))
        .join(machine.id(&root))
        .join("daemon.sock");
    assert!(socket.exists(), "{}", socket.display());
    assert_eq!(tay(&["down"]).code, 0);
}
