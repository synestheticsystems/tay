//! `port` units: a number no other environment of tay on this machine is
//! given, tested as free when chosen, and remembered. The preferred number
//! is tried once, the first time; after that the environment keeps what it
//! got; other numbers come from 20000 to 29999; a kept number taken by
//! something else is chosen again, and a fixed one fails its unit.

use std::collections::BTreeSet;
use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::time::Duration;

use crate::harness::{Machine, kept_ports};

fn number(root: &Path, unit: &str) -> u16 {
    kept_ports(root)[unit].as_u64().unwrap() as u16
}

/// A port no program listens on now, outside the range tay chooses from.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
fn the_preferred_number_is_tried_once_and_after_that_the_kept_one_wins() {
    let machine = Machine::new();
    let root = machine.stack("ports");
    let preferred = free_port().to_string();
    let set = format!("settings.preferred={preferred}");
    let up = machine.at(&root, "up", &["--set", &set]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    // One unit gets it; the others that prefer it are given numbers from
    // the range, and say so.
    let preferring = ["one", "two", "three", "four"];
    let had: Vec<&&str> = preferring
        .iter()
        .filter(|unit| number(&root, unit).to_string() == preferred)
        .collect();
    assert_eq!(had.len(), 1, "{:?}", kept_ports(&root));
    assert_eq!(
        up.last_change(had[0]),
        format!("ready {preferred}, as preferred")
    );
    for unit in preferring.iter().filter(|unit| !had.contains(unit)) {
        let given = number(&root, unit);
        assert_eq!(
            up.last_change(unit),
            format!(
                "ready {given}: {preferred}, which it prefers, is kept by unit `{}`",
                had[0]
            )
        );
    }
    let before = kept_ports(&root);
    // Preferring another number later changes nothing: kept beats preferred.
    let other = format!("settings.preferred={}", free_port());
    let again = machine.at(&root, "up", &["--set", &other]);
    assert_eq!(again.code, 0, "{}", again.stderr);
    assert_eq!(kept_ports(&root), before);
    assert_eq!(
        again.last_change("one"),
        format!("ready {}, kept", number(&root, "one"))
    );
}

#[test]
fn numbers_tay_chooses_are_from_20000_to_29999_and_each_is_free() {
    let machine = Machine::new();
    let root = machine.stack("ports");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    for unit in ["five", "six", "seven", "eight"] {
        let number = number(&root, unit);
        assert!((20000..=29999).contains(&number), "{unit}: {number}");
    }
    let numbers: BTreeSet<u64> = kept_ports(&root)
        .values()
        .map(|n| n.as_u64().unwrap())
        .collect();
    assert_eq!(numbers.len(), 8, "every unit has a number of its own");
}

#[test]
fn two_environments_brought_up_at_once_never_share_a_port() {
    let machine = Machine::new();
    let roots: Vec<_> = (0..4)
        .map(|i| machine.stack_as("ports", &format!("ports-{i}")))
        .collect();
    let ups: Vec<_> = roots
        .iter()
        .map(|root| {
            let mut up = machine.command(root, &["up"]);
            up.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            up.spawn().unwrap()
        })
        .collect();
    for up in ups {
        let output = up.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut every = BTreeSet::new();
    for root in &roots {
        for number in kept_ports(root).values() {
            assert!(
                every.insert(number.as_u64().unwrap()),
                "{number} is given twice"
            );
        }
    }
    assert_eq!(every.len(), 32);
    let listed: BTreeSet<u64> = machine.list()["environments"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|listed| listed["ports"].as_array().unwrap().iter())
        .map(|number| number.as_u64().unwrap())
        .collect();
    assert_eq!(listed, every);
}

#[test]
fn a_kept_port_taken_by_something_else_is_chosen_again_and_up_says_so() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let kept_number = number(&root, "config-port");
    let holder = TcpListener::bind(("127.0.0.1", kept_number)).unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let chosen = number(&root, "config-port");
    assert_ne!(chosen, kept_number);
    assert_eq!(
        up.last_change("config-port"),
        format!(
            "ready {chosen}, chosen again: {kept_number}, which it kept, is held by a program that is not this environment's"
        )
    );
    // The file built from it is written again with the new number.
    let config = fs::read_to_string(root.join(".tay/state/files/config/config")).unwrap();
    assert_eq!(config, format!("port: {chosen}\n"));
    drop(holder);
}

#[test]
fn a_fixed_port_that_is_taken_fails_its_unit_and_names_the_holder() {
    let machine = Machine::new();
    let first = machine.stack_as("fixed-port", "fixed-port");
    let second = machine.stack_as("fixed-port", "fixed-port-again");
    let number = free_port().to_string();
    let set = format!("settings.number={number}");
    let up = machine.at(&first, "up", &["--set", &set]);
    assert_eq!(up.code, 0, "{}", up.stderr);
    assert_eq!(up.last_change("listener-port"), format!("ready {number}"));
    // Another environment keeps it.
    let up = machine.at(&second, "up", &["--set", &set]);
    assert_eq!(up.code, 1);
    assert_eq!(
        up.last_change("listener-port"),
        format!(
            "failed it must have {number}, which is kept by the environment at `{}`",
            first.display()
        )
    );
    // A program that is not tay's holds another.
    let holder = TcpListener::bind("127.0.0.1:0").unwrap();
    let held = holder.local_addr().unwrap().port().to_string();
    let up = machine.at(
        &second,
        "up",
        &["--set", &format!("settings.number={held}")],
    );
    assert_eq!(up.code, 1);
    assert_eq!(
        up.last_change("listener-port"),
        format!(
            "failed it must have {held}, which is held by a program that is not this environment's"
        )
    );
}

#[test]
fn a_port_the_environments_own_process_listens_on_is_not_taken_from_it() {
    let machine = Machine::new();
    let root = machine.stack("checks");
    assert_eq!(machine.at(&root, "up", &["listener"]).code, 0);
    let number = kept_ports(&root)["listener-port"].clone();
    let listener = crate::harness::child(&root, "listener");
    // At the next `tay up` the kept number is held by the environment's own
    // process: it is kept, not chosen again.
    let again = machine.at(&root, "up", &["listener"]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert_eq!(
        again.last_change("listener-port"),
        format!("ready {number}, kept")
    );
    assert_eq!(kept_ports(&root)["listener-port"], number);
    assert_eq!(crate::harness::child(&root, "listener"), listener);
    // Nor does a fixed one fail its unit.
    let free = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let set = format!("settings.fixed={free}");
    let pinned = machine.at(&root, "up", &["pinned", "--set", &set]);
    assert_eq!(pinned.code, 0, "{}{}", pinned.stdout, pinned.stderr);
    let again = machine.at(&root, "up", &["pinned"]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert_eq!(again.last_change("fixed-port"), format!("ready {free}"));
}

#[test]
fn a_port_the_environments_own_process_holds_through_a_file_is_not_taken_from_it() {
    let machine = Machine::new();
    let root = machine.stack("checks");
    assert_eq!(machine.at(&root, "up", &["reader"]).code, 0);
    let number = kept_ports(&root)["conf-port"].clone();
    // `reader` listens on the number its file gives, and uses the port
    // only through the file.
    assert!(crate::harness::eventually(Duration::from_secs(20), || {
        std::net::TcpStream::connect(("127.0.0.1", number.as_u64().unwrap() as u16)).is_ok()
    }));
    let again = machine.at(&root, "up", &["reader"]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert_eq!(
        again.last_change("conf-port"),
        format!("ready {number}, kept")
    );
    assert_eq!(kept_ports(&root)["conf-port"], number);
}

#[test]
fn a_change_that_brings_up_part_keeps_a_port_another_unit_of_the_environment_listens_on() {
    let machine = Machine::new();
    let root = machine.stack("held-ports");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let kept = number(&root, "shared-port");
    // `client` uses the number, and does not need `server`, which listens
    // on it and which neither change touches.
    let restart = machine.at(&root, "restart", &["client"]);
    assert_eq!(restart.code, 0, "{}{}", restart.stdout, restart.stderr);
    assert_eq!(
        restart.last_change("shared-port"),
        format!("ready {kept}, kept")
    );
    let run = machine.at(&root, "run", &["show"]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, format!("{kept}\n"));
    assert_eq!(number(&root, "shared-port"), kept);
}

#[test]
fn a_port_its_own_unit_stopped_listening_on_is_kept_whatever_its_closed_connections_leave() {
    let machine = Machine::new();
    let root = machine.stack("held-ports");
    let up = machine.at(&root, "up", &["own"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let kept = number(&root, "own-port");
    // A connection from outside the environment, which `own` closes as it
    // is stopped, leaving its side of it holding the number for a while.
    let mut connection = std::net::TcpStream::connect(("127.0.0.1", kept)).unwrap();
    let held = crate::harness::eventually(Duration::from_secs(30), || {
        crate::harness::log(&root, "own").contains("holding a connection")
    });
    assert!(held, "{}", crate::harness::log(&root, "own"));
    let closing = std::thread::spawn(move || {
        let mut rest = Vec::new();
        std::io::Read::read_to_end(&mut connection, &mut rest)
    });
    let restart = machine.at(&root, "restart", &["own"]);
    assert_eq!(restart.code, 0, "{}{}", restart.stdout, restart.stderr);
    assert_eq!(
        restart.last_change("own-port"),
        format!("ready {kept}, kept")
    );
    // It listens on its number again.
    let own = restart.last_change("own");
    assert!(own.starts_with("ready "), "{own}");
    closing.join().unwrap().unwrap();
}
