//! A container's readiness: a `command` check runs inside it, an `http` or
//! `tcp` check is made from the host through the port the plan opens, and
//! a `log` check reads what it wrote. Its image's own health check is in
//! `bringing_up.rs`.

use crate::harness::Machine;

#[test]
fn a_command_check_runs_inside_the_container() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["marked"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(
        up.changes().contains(
            &"marked starting attempt 1, waiting to be ready: command `test -f /tmp/marker`"
                .to_string()
        ),
        "{}",
        up.stdout
    );
    assert_eq!(
        up.last_change("marked"),
        "ready attempt 1, command `test -f /tmp/marker` passes"
    );
}

#[test]
fn a_log_check_reads_what_the_container_wrote() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["listening"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(
        up.last_change("listening"),
        "ready attempt 1, log \"listening\" passes"
    );
}

#[test]
fn a_tcp_check_is_made_from_the_host_through_the_opened_port() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["served"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let port = crate::harness::kept_ports(&root)["box-port"]
        .as_u64()
        .unwrap();
    assert_eq!(
        up.last_change("served"),
        format!("ready attempt 1, tcp 127.0.0.1:{port} passes")
    );
}
