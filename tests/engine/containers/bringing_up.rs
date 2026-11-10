//! Bringing a container up: its image pulled if it is not here or built,
//! saying so in its detail; its volumes made and given to their owner; and
//! the container created from every setting the format lists, a published
//! port given to Docker only when the plan opens it.

use super::container;
use crate::harness::{Machine, log};

#[test]
fn an_image_that_is_not_here_is_pulled_saying_so_and_one_no_registry_has_fails_the_unit() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["missing-image"]);
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    assert!(
        up.changes().contains(
            &"missing-image starting attempt 1, pulling `busybox:tay-test-no-such-tag`".to_string()
        ),
        "{}",
        up.stdout
    );
    let failed = up.last_change("missing-image");
    assert!(
        failed.starts_with("failed Docker could not pull `busybox:tay-test-no-such-tag`: "),
        "{failed}"
    );
    assert!(container(&machine, &root, "missing-image").is_none());
}

#[test]
fn a_built_image_is_built_saying_so_and_is_ready_when_its_own_health_check_passes() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["healthy"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let tag = format!("tay-{}-healthy", machine.id(&root));
    let changes = up.changes();
    assert!(
        changes.contains(&format!("healthy starting attempt 1, building `{tag}`")),
        "{}",
        up.stdout
    );
    assert!(
        changes.contains(
            &"healthy starting attempt 1, waiting to be ready: its image's health check has not passed yet"
                .to_string()
        ),
        "{}",
        up.stdout
    );
    assert_eq!(
        up.last_change("healthy"),
        "ready attempt 1, its image's health check passes"
    );
    // What the build said is in the unit's log, among tay's notes.
    let log = log(&root, "healthy");
    assert!(log.contains(" # build: "), "{log}");
    assert!(log.contains(" | healthy now"), "{log}");
    assert_eq!(
        container(&machine, &root, "healthy").unwrap()["Config"]["Image"],
        tag.as_str()
    );
}

#[test]
fn a_container_is_created_from_every_setting_it_has() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["shaped"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(crate::harness::eventually(
        std::time::Duration::from_secs(20),
        || log(&root, "shaped").contains("scratch writable")
    ));
    let log = log(&root, "shaped");
    for said in [
        "| user 1234 in /tmp",
        "| root read-only",
        "| scratch writable",
    ] {
        assert!(log.contains(said), "{said}: {log}");
    }
    let shaped = container(&machine, &root, "shaped").unwrap();
    let (config, host) = (&shaped["Config"], &shaped["HostConfig"]);
    assert_eq!(config["Entrypoint"], serde_json::json!(["sh", "-c"]));
    assert_eq!(config["User"], "1234");
    assert_eq!(config["WorkingDir"], "/tmp");
    assert_eq!(config["Labels"]["team"], "boxes");
    assert_eq!(host["ReadonlyRootfs"], true);
    assert_eq!(
        host["SecurityOpt"],
        serde_json::json!(["no-new-privileges"])
    );
    assert_eq!(host["PidsLimit"], 64);
    assert!(host["Tmpfs"]["/scratch"].is_string(), "{host}");
    // No restart policy of Docker's: tay restarts what ends.
    assert!(
        matches!(host["RestartPolicy"]["Name"].as_str(), Some("" | "no")),
        "{host}"
    );
}

#[test]
fn a_published_port_is_given_to_docker_only_when_the_plan_opens_it() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["web", "quiet"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let port = crate::harness::kept_ports(&root)["web-port"]
        .as_u64()
        .unwrap();
    let web = container(&machine, &root, "web").unwrap();
    assert_eq!(
        web["HostConfig"]["PortBindings"],
        serde_json::json!({ "8080/tcp": [{ "HostIp": "127.0.0.1", "HostPort": port.to_string() }] })
    );
    // Its own check reached it from the host, through that port.
    assert_eq!(
        up.last_change("web"),
        format!("ready attempt 1, http http://127.0.0.1:{port}/ passes")
    );
    let quiet = container(&machine, &root, "quiet").unwrap();
    let bound = &quiet["HostConfig"]["PortBindings"];
    assert!(
        bound.is_null() || bound.as_object().is_some_and(|ports| ports.is_empty()),
        "{bound}"
    );
    // The port that is not opened brings in nothing it would use.
    assert!(!crate::harness::kept_ports(&root).contains_key("quiet-port"));
}

#[test]
fn a_volume_is_made_and_given_to_its_owner() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["owned"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(crate::harness::eventually(
        std::time::Duration::from_secs(20),
        || log(&root, "owned").contains("| owner 4321")
    ));
    let id = machine.id(&root);
    let volume = crate::docker::inspect(&format!("tay-{id}-owned.data")).unwrap();
    assert_eq!(volume["Labels"]["tay.unit"], "owned");
    // What gave it to its owner is gone.
    assert!(crate::docker::inspect(&format!("tay-{id}-owned.owner")).is_none());
}
