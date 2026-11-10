//! An address depends on who asks: the view of an endpoint follows where the
//! unit that uses it runs, unless the reference asks for one.

mod support;

use support::{given, plan, plan_json, unit};

/// The words of a unit's readiness command, as resolved for that unit.
fn probe(plan: &serde_json::Value, name: &str) -> Vec<String> {
    unit(plan, name)["ready"]["target"]
        .as_array()
        .unwrap()
        .iter()
        .map(|word| word["text"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_unit_on_the_host_sees_127_0_0_1_and_host_ports() {
    let plan = plan_json("endpoints", &[]);
    assert_eq!(
        probe(&plan, "host-user"),
        [
            "probe",
            // A process's endpoint: 127.0.0.1 and its port.
            "http://127.0.0.1:<port: host-port>/v1",
            // A container's endpoint: 127.0.0.1 and the published host port.
            "127.0.0.1:<port: box-port>",
            // `@net` asks for the network's view whoever asks.
            "http://in-box:8080",
            // An address carried in another unit's value takes this unit's view.
            "http://127.0.0.1:<port: box-port>/v2",
            "127.0.0.1",
            "<port: box-port>",
        ]
    );
}

#[test]
fn a_unit_in_a_container_sees_the_host_by_name_and_containers_by_unit_name() {
    let plan = plan_json("endpoints", &[]);
    assert_eq!(
        probe(&plan, "box-user"),
        [
            "probe",
            // A process's endpoint: the name containers have for the host.
            "host.docker.internal:<port: host-port>",
            // A container's endpoint: its unit's name and the port inside it.
            "http://in-box:8080",
            // `@host` asks for the host's view whoever asks.
            "http://127.0.0.1:<port: host-port>/v1",
            "http://in-box:8080/v2",
            "host.docker.internal",
        ]
    );
}

#[test]
fn the_plan_shows_each_endpoint_from_the_host_and_from_a_container() {
    let plan = plan_json("endpoints", &[]);
    let api = given(unit(&plan, "in-box"), "api");
    assert_eq!(api["endpoint"], true);
    assert_eq!(
        api["from_host"]["text"],
        "http://127.0.0.1:<port: box-port>"
    );
    assert_eq!(api["from_containers"]["value"], "http://in-box:8080");
}

#[test]
fn an_unpublished_container_port_used_from_the_host_is_an_error_naming_both_units() {
    let run = plan("endpoint-errors", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:13:18: unit `host-user` runs on the host, and uses here an address of unit `hidden` as the host sees it, but `hidden` publishes no host port for its port 5432",
            "  tay.yaml:8:11: the endpoint of `hidden` is defined here",
            // From a container the same address is fine, unless the host's view is asked for.
            "tay.yaml:18:43: unit `box-user` runs in a container, and uses here an address of unit `hidden` as the host sees it, but `hidden` publishes no host port for its port 5432",
            "  tay.yaml:8:11: the endpoint of `hidden` is defined here",
            // A file is read by whoever reads it, so it must say which view.
            "tay.yaml:21:22: unit `config` is made, and runs nowhere, so `${hidden.db}` must say which view of the address it wants: add `@host` or `@net`",
            "  tay.yaml:8:11: the endpoint is defined here",
        ]
    );
}
