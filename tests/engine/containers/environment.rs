//! What belongs to an environment in Docker: one network, `tay-<id>`, and
//! every container, volume and network tay creates labelled with the
//! environment's id and, but for the network, its unit. Nothing without
//! that label is ever stopped, removed or changed. `tay down` removes the
//! containers and the network and keeps the volumes.

use std::time::Duration;

use super::{container, labelled, name};
use crate::docker::{docker, inspect};
use crate::harness::{Machine, eventually};

#[test]
fn what_tay_creates_carries_the_environments_label_and_a_container_answers_to_its_unit_and_aliases()
{
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["shaped", "keeper-data"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let id = machine.id(&root);
    let shaped = container(&machine, &root, "shaped").unwrap();
    let labels = &shaped["Config"]["Labels"];
    assert_eq!(labels["tay.environment"], id.as_str());
    assert_eq!(labels["tay.unit"], "shaped");
    let network = format!("tay-{id}");
    let aliases = shaped["NetworkSettings"]["Networks"][&network]["Aliases"]
        .as_array()
        .unwrap_or_else(|| panic!("`shaped` is on `{network}`: {shaped}"));
    for alias in ["shaped", "shape"] {
        assert!(aliases.iter().any(|known| known == alias), "{aliases:?}");
    }
    let made = inspect(&network).unwrap();
    assert_eq!(made["Labels"]["tay.environment"], id.as_str());
    let volume = inspect(&format!("tay-{id}-keeper-data.data")).unwrap();
    assert_eq!(volume["Labels"]["tay.environment"], id.as_str());
    assert_eq!(volume["Labels"]["tay.unit"], "keeper-data");
    let [containers, volumes, networks] = labelled(&machine, &root);
    assert_eq!(
        containers,
        [
            name(&machine, &root, "keeper-data"),
            name(&machine, &root, "shaped")
        ]
    );
    assert_eq!(volumes, [format!("tay-{id}-keeper-data.data")]);
    assert_eq!(networks, [network]);
}

#[test]
fn two_environments_up_at_once_share_nothing_and_down_in_one_leaves_the_other_ready() {
    let machine = Machine::with_docker();
    let one = machine.stack_as("containers", "boxes-one");
    let two = machine.stack_as("containers", "boxes-two");
    let ups = [
        machine.spawn_up(&one, &["web", "keeper-data"]),
        machine.spawn_up(&two, &["web", "keeper-data"]),
    ];
    for up in ups {
        let up = up.finish();
        assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    }
    let [ones, twos] = [labelled(&machine, &one), labelled(&machine, &two)];
    for (one, two) in ones.iter().zip(&twos) {
        assert_eq!(one.len(), two.len());
        assert!(
            one.iter().all(|name| !two.contains(name)),
            "{one:?} {two:?}"
        );
    }
    let port = |root| {
        container(&machine, root, "web").unwrap()["HostConfig"]["PortBindings"]["8080/tcp"][0]
            ["HostPort"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_ne!(port(&one), port(&two));
    let down = machine.at(&one, "down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    let status = machine.at(&two, "status", &["--json"]).json();
    for unit in ["web", "keeper-data"] {
        assert!(
            Machine::state(&status, unit).starts_with("ready "),
            "{unit}: {status}"
        );
    }
    assert_eq!(labelled(&machine, &two), twos);
}

#[test]
fn down_leaves_no_container_or_network_and_keeps_the_volumes() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let up = machine.at(&root, "up", &["keeper-data", "ticker"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    assert_eq!(
        down.stdout.lines().last(),
        Some("tay down: nothing is left running.")
    );
    let [containers, volumes, networks] = labelled(&machine, &root);
    assert!(containers.is_empty(), "{containers:?}");
    assert!(networks.is_empty(), "{networks:?}");
    assert_eq!(
        volumes,
        [format!("tay-{}-keeper-data.data", machine.id(&root))]
    );
}

/// A container and a network made by the test itself, with no label of
/// tay's, removed when the test ends, also when it fails.
struct Foreign {
    container: String,
    network: Option<String>,
}

impl Drop for Foreign {
    fn drop(&mut self) {
        docker(&["rm", "-f", &self.container]);
        if let Some(network) = &self.network {
            docker(&["network", "rm", network]);
        }
    }
}

#[test]
fn nothing_without_the_environments_label_is_touched_even_with_a_name_like_tays() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    // The environment's id is chosen here, before its first `tay up`, so
    // that the network and container a test makes can take its names.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let id = format!("{:012x}", nanos & 0xffff_ffff_ffff);
    std::fs::create_dir_all(root.join(".tay")).unwrap();
    std::fs::write(root.join(".tay/id"), format!("{id}\n")).unwrap();
    let network = format!("tay-{id}");
    let squatter = format!("tay-{id}-squatter");
    let mut foreign = Foreign {
        container: squatter.clone(),
        network: Some(network.clone()),
    };
    docker(&["network", "create", &network]);
    docker(&[
        "run",
        "-d",
        "--network",
        "none",
        "--memory",
        "16m",
        "--name",
        &squatter,
        "busybox:1.36",
        "sleep",
        "600",
    ]);
    let started = inspect(&squatter).unwrap()["Id"].clone();
    let up = machine.at(&root, "up", &["ticker"]);
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    assert_eq!(
        up.last_change("ticker"),
        format!(
            "failed a network named `{network}` exists, and tay did not create it, so tay leaves it alone"
        )
    );
    // With that network gone, tay makes its own; the container named as
    // `squatter`'s would be is left alone.
    docker(&["network", "rm", &network]);
    foreign.network = None;
    let up = machine.at(&root, "up", &["squatter"]);
    assert_eq!(up.code, 1, "{}{}", up.stdout, up.stderr);
    assert_eq!(
        up.last_change("squatter"),
        format!(
            "failed a container named `{squatter}` exists, and tay did not create it, so tay leaves it alone"
        )
    );
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    let after = inspect(&squatter).unwrap();
    assert_eq!(after["Id"], started);
    assert_eq!(after["State"]["Running"], true);
    assert!(eventually(Duration::from_secs(10), || inspect(&network).is_none()));
    drop(foreign);
}
