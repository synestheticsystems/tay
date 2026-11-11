//! What belongs to an environment in Docker: one network, `tay-<id>`, and
//! every container, volume and network tay creates labelled with the
//! environment's id and, but for the network, its unit. Nothing without
//! that label is ever stopped, removed or changed. `tay down` removes the
//! containers and the network and keeps the volumes, but for those Docker
//! made by itself for one container, which go with it whenever it is
//! removed.

use std::time::Duration;

use super::{container, labelled, name};
use crate::docker::{docker, inspect};
use crate::harness::{Machine, eventually, log};

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
    // The machine's list notes that Docker holds something of it.
    assert_eq!(machine.list()["environments"][0]["made_outside"], true);
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

#[test]
fn a_volume_docker_made_for_an_image_goes_with_its_container_and_the_units_own_volume_stays() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    let started = |start: u32| {
        let said = format!(", start {start}\n");
        assert!(
            eventually(Duration::from_secs(20), || log(&root, "cached")
                .contains(&said)),
            "{}",
            log(&root, "cached")
        );
    };
    // The volume Docker made by itself for the path the image declares,
    // which the unit does not name.
    let made_for_the_image = || {
        let cached = container(&machine, &root, "cached").unwrap();
        let mounts = cached["Mounts"].as_array().unwrap().clone();
        let cache = mounts.iter().find(|mount| mount["Destination"] == "/cache");
        cache.unwrap()["Name"].as_str().unwrap().to_string()
    };
    let up = machine.at(&root, "up", &["cached"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    started(1);
    let first = made_for_the_image();
    // Replaced, the container goes with it.
    let replaced = machine.at(&root, "up", &["cached", "--set", "settings.mode=two"]);
    assert_eq!(replaced.code, 0, "{}{}", replaced.stdout, replaced.stderr);
    started(2);
    assert!(inspect(&first).is_none(), "`{first}` is left");
    let second = made_for_the_image();
    assert_ne!(second, first);
    // Taken down, likewise.
    let down = machine.at(&root, "down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    assert!(inspect(&second).is_none(), "`{second}` is left");
    // The unit's own volume stays, and what it holds: it counts every start.
    let [_, volumes, _] = labelled(&machine, &root);
    assert_eq!(volumes, [format!("tay-{}-cached.data", machine.id(&root))]);
    assert_eq!(machine.at(&root, "up", &["cached"]).code, 0);
    started(3);
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

#[test]
fn an_environment_leaves_the_machines_list_only_when_nothing_of_it_is_left_in_docker() {
    let machine = Machine::with_docker();
    let root = machine.stack("containers");
    assert_eq!(machine.at(&root, "up", &["keeper-data"]).code, 0);
    assert_eq!(machine.at(&root, "down", &[]).code, 0);
    let id = machine.id(&root);
    std::fs::remove_dir_all(&root).unwrap();
    // Its volume is kept, so it stays, its folder missing.
    let ls = machine.tay(&machine.path("home"), &["ls"]);
    assert!(
        ls.stdout.contains(&id) && ls.stdout.contains("its folder is missing"),
        "{}",
        ls.stdout
    );
    docker(&["volume", "rm", &format!("tay-{id}-keeper-data.data")]);
    let ls = machine.tay(&machine.path("home"), &["ls"]);
    assert_eq!(ls.stdout, "tay knows no environment on this machine.\n");
}

#[test]
fn an_environment_that_never_ran_a_container_never_asks_docker() {
    let machine = Machine::new();
    let root = machine.stack("waiting");
    // Docker's configuration points where nothing answers, and tay talks
    // to no Docker there.
    let tay = |command: &str| {
        let mut run = machine.command(&root, &[command, "--root", root.to_str().unwrap()]);
        run.env("DOCKER_HOST", "tcp://127.0.0.1:9");
        machine.run(run)
    };
    for command in ["up", "status", "down", "ls"] {
        let run = tay(command);
        assert_eq!(run.code, 0, "{command}: {}{}", run.stdout, run.stderr);
    }
    let list = machine.list();
    assert_eq!(list["environments"][0]["made_outside"], false, "{list}");
}

#[test]
fn a_host_process_listens_on_the_loopback_address_and_needs_no_docker_when_no_container_runs() {
    let machine = Machine::new();
    let root = machine.stack("checks");
    // Docker's configuration names a socket that does not exist.
    let socket = format!("unix://{}", machine.path("no-docker.sock").display());
    let tay = |command: &str, args: &[&str]| {
        let mut all = vec![command, "--root", root.to_str().unwrap()];
        all.extend(args);
        let mut run = machine.command(&root, &all);
        run.env("DOCKER_HOST", &socket);
        machine.run(run)
    };
    // `site` listens on `${host.listen}`, and its own check reaches it
    // there from the host.
    let up = tay("up", &["site"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let port = crate::harness::kept_ports(&root)["site-port"].clone();
    assert_eq!(
        up.last_change("site"),
        format!("ready attempt 1, http http://127.0.0.1:{port}/health passes")
    );
    let down = tay("down", &[]);
    assert_eq!(down.code, 0, "{}{}", down.stdout, down.stderr);
    let list = machine.list();
    assert_eq!(list["environments"][0]["made_outside"], false, "{list}");
}
