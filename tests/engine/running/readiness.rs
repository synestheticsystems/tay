//! Readiness, exactly: an `http` GET that gets any 2xx answer, a `tcp`
//! connection that opens, a `command` that exits 0 or a line the unit wrote
//! that has the text, within the unit's own `within`; checked again once the
//! unit is ready, so that `unready` and a return to `ready` are both seen.

use std::fs;
use std::time::Duration;

use crate::harness::Machine;

#[test]
fn an_http_check_passes_on_a_2xx_answer_and_a_ready_unit_whose_check_stops_passing_is_unready() {
    let machine = Machine::new();
    let root = machine.stack("checks");
    let up = machine.at(&root, "up", &["site"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let changes = up.changes();
    // It prepares before its first start, and is ready by its check.
    assert!(
        changes.iter().any(
            |line| line.starts_with("site starting attempt 1, preparing: `")
                && line.ends_with("/healthy`")
        ),
        "{}",
        up.stdout
    );
    // The check is made where the process listens, at the address
    // `${host.listen}` names.
    let ready = up.last_change("site");
    let port = crate::harness::kept_ports(&root)["site-port"].clone();
    assert!(
        ready.starts_with("ready attempt 1, http http://")
            && ready.ends_with(&format!(":{port}/health passes")),
        "{ready}"
    );
    let healthy = root.join(".tay/state/healthy");
    fs::remove_file(&healthy).unwrap();
    let unready = machine.until_state(&root, "site", Duration::from_secs(15), |state| {
        state.starts_with("unready")
    });
    assert!(
        unready.ends_with("/health no longer passes: it answered 503 Service Unavailable"),
        "{unready}"
    );
    fs::write(&healthy, "").unwrap();
    let again = machine.until_state(&root, "site", Duration::from_secs(15), |state| {
        state.starts_with("ready")
    });
    assert!(again.ends_with("/health passes again"), "{again}");
    // Still the first attempt: being unready is not ending.
    assert!(again.starts_with("ready attempt 1,"), "{again}");
}

#[test]
fn a_tcp_check_passes_when_a_connection_opens_and_a_command_check_when_it_exits_0() {
    let machine = Machine::new();
    let root = machine.stack("checks");
    let up = machine.at(&root, "up", &["listener", "probed"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let listener = up.last_change("listener");
    assert!(
        listener.starts_with("ready attempt 1, tcp 127.0.0.1:") && listener.ends_with(" passes"),
        "{listener}"
    );
    let probed = up.last_change("probed");
    assert!(
        probed.starts_with("ready attempt 1, command `")
            && probed.ends_with("/probed-ready` passes"),
        "{probed}"
    );
    // The command check could not have passed before the unit wrote its file.
    assert!(root.join(".tay/state/probed-ready").exists());
}

#[test]
fn an_http_check_of_an_https_address_fails_saying_tay_cannot_make_it_yet() {
    let machine = Machine::new();
    let root = machine.stack("checks");
    let up = machine.at(&root, "up", &["secure"]);
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    let secure = up.last_change("secure");
    assert!(
        secure.starts_with("failed its check `http https://127.0.0.1:")
            && secure
                .ends_with("is of an `https` address, and tay cannot make an `https` check yet"),
        "{secure}"
    );
}

#[test]
fn a_ready_unit_found_again_after_its_within_has_passed_is_judged_as_a_ready_unit() {
    let machine = Machine::new();
    let root = machine.stack("checks");
    let up = machine.at(&root, "up", &["brief"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let brief = crate::harness::child(&root, "brief");
    std::thread::sleep(Duration::from_secs(9));
    machine.kill_daemon(&root);
    let found = machine.until_state(&root, "brief", Duration::from_secs(20), |state| {
        !state.starts_with("starting")
    });
    assert!(
        found.starts_with("ready attempt 1, ready when it was found again; http http://127.0.0.1:")
            && found.ends_with("/health is checked again"),
        "{found}"
    );
    assert_eq!(
        crate::harness::child(&root, "brief"),
        brief,
        "it was not started again"
    );
    // One that was ready and whose check fails now is unready, not stopped.
    machine.kill_daemon(&root);
    fs::remove_file(root.join(".tay/state/healthy")).unwrap();
    let unready = machine.until_state(&root, "brief", Duration::from_secs(20), |state| {
        state.starts_with("unready")
    });
    assert!(
        unready.ends_with("it answered 503 Service Unavailable"),
        "{unready}"
    );
    assert_eq!(crate::harness::child(&root, "brief"), brief);
}
