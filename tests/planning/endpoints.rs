//! An address depends on who asks: the view of an endpoint follows where the
//! unit that uses it runs, unless the reference asks for one.

use crate::support::{given, plan, plan_json, unit, unit_names};
use serde_json::json;

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
            "tay.yaml:21:28: unit `config` is made, and runs nowhere, so `${hidden.db}` must say which view of the address it wants: add `@host`, `@net`, or `@` and the name of the unit that uses it",
            "  tay.yaml:8:11: the endpoint is defined here",
            // Every address in one text that its user cannot have is
            // reported, each at the reference that uses it.
            "tay.yaml:29:29: unit `both-user` runs on the host, and uses here an address of unit `hidden` as the host sees it, but `hidden` publishes no host port for its port 5432",
            "  tay.yaml:8:11: the endpoint of `hidden` is defined here",
            "tay.yaml:29:46: unit `both-user` runs on the host, and uses here an address of unit `also-hidden` as the host sees it, but `also-hidden` publishes no host port for its port 6543",
            "  tay.yaml:26:11: the endpoint of `also-hidden` is defined here",
            // A unit that is made gives an endpoint whose host needs a view.
            "tay.yaml:33:22: a value of a unit that is made, which runs nowhere, holds it, so `${hidden.db.host}` must say which view of the address it wants: add `@host`, `@net`, or `@` and the name of the unit that uses it",
            "  tay.yaml:8:11: the endpoint is defined here",
        ]
    );
}

#[test]
fn a_view_named_after_a_unit_is_the_address_as_that_unit_sees_it_under_its_way() {
    let plan = plan_json("views", &[]);
    assert_eq!(probe(&plan, "setup"), ["check", "http://wiki:3000"]);
    let plan = plan_json("views", &["--set", "run.units.proxy=source"]);
    assert_eq!(
        probe(&plan, "setup"),
        ["check", "http://127.0.0.1:<port: wiki-port>"]
    );
}

#[test]
fn a_unit_with_ways_needs_a_usable_way_only_when_the_plan_reaches_it() {
    // `lonely` has no way `run.default` names; nothing that comes up uses it
    // until `asker` asks for its view.
    assert_eq!(plan("views", &[]).code, 0);
    let run = plan("views", &["--set", "up_add=asker"]);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:16:3: unit `lonely` has the ways `alpha`, `beta`, and no choice names one: `run.units` does not name it, and `run.default` (`image`) names none of them",
            "  tay.yaml:4:18: `run.default` is chosen here",
        ]
    );
}

#[test]
fn an_endpoint_that_writes_its_host_is_the_same_for_everyone_and_a_url_leaves_out_a_usual_port() {
    // `notes`, a file, uses the portal's address with no view: it has none.
    let plan = plan_json("views", &[]);
    let portal = unit(&plan, "portal");
    let url = |name: &str| {
        let value = given(portal, name);
        assert_eq!(value["from_host"], value["from_containers"]);
        value["from_host"]["value"].clone()
    };
    assert_eq!(url("public"), "https://portal.example.test");
    assert_eq!(url("alternate"), "https://alternate.example.test:8443");
    assert_eq!(url("plain"), "http://plain.example.test/start");
    // Only `http` and `https` have a usual port.
    assert_eq!(url("socket"), "ws://socket.example.test:80");
}

#[test]
fn a_view_must_name_a_unit_that_runs_a_url_needs_a_scheme_and_a_made_unit_writes_its_host() {
    let run = plan("view-errors", &[]);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:12:23: `${settings.nosuch}`: no setting `nosuch` is declared in tay.yaml",
            "tay.yaml:16:19: `${wiki.web.url@wiki-port}` asks for the view of unit `wiki-port`, which runs nowhere; the unit named after `@` must be one that runs",
            "tay.yaml:16:48: `${wiki.web.url@nobody}`: there is no unit `nobody`",
            "tay.yaml:16:74: `${wiki.raw.url}`: the endpoint has no `scheme`, so it has no URL",
            "tay.yaml:20:12: unit `held` is made and runs nothing that listens, so an endpoint it gives must write its `host`",
        ]
    );
}

#[test]
fn a_published_port_is_opened_only_for_a_use_from_the_host_or_always_and_otherwise_uses_nothing() {
    let plan = plan_json("published", &[]);
    // Nothing opens 9090, so the port unit named only there stays out.
    let names = unit_names(&plan);
    assert!(!names.contains(&"quiet-port"), "{names:?}");
    assert!(names.contains(&"always-port"), "{names:?}");
    let box_unit = unit(&plan, "box");
    assert_eq!(
        box_unit["publishes"],
        json!([
            {
                "container_port": 8080,
                "opened": true,
                "address": { "text": "127.0.0.1", "value": "127.0.0.1" },
                "host_port": { "text": "<port: box-port>", "value": null },
                "always": false,
                "opened_by": [
                    { "unit": "box", "how": "readiness_check" },
                    { "unit": "web", "how": "unit_on_host" },
                ],
            },
            { "container_port": 9090, "opened": false },
            {
                "container_port": 7070,
                "opened": true,
                "address": { "text": "0.0.0.0", "value": "0.0.0.0" },
                "host_port": { "text": "<port: always-port>", "value": null },
                "always": true,
                "opened_by": [],
            },
        ])
    );
    assert_eq!(
        given(box_unit, "quiet")["from_host"],
        json!({ "unreachable": "its port 9090 is published, but nothing in the plan opens it on the host" })
    );
    let text = crate::support::plan("published", &[]).stdout;
    assert!(text.contains(
        "   publishes 8080 at 127.0.0.1:<port: box-port>, opened for box (its readiness check), web (on the host)\n"
    ));
    assert!(text.contains(
        "   publishes 9090: not opened, since nothing in the plan uses it from the host\n"
    ));
    assert!(text.contains("   publishes 7070 at 0.0.0.0:<port: always-port>, opened for always\n"));
}

#[test]
fn a_view_of_the_host_opens_a_published_port_for_a_unit_in_a_container() {
    let plan = plan_json("published", &["--set", "up_add=viewer"]);
    assert_eq!(
        unit(&plan, "box")["publishes"][0]["opened_by"][1],
        json!({ "unit": "viewer", "how": "view" })
    );
}

#[test]
fn each_part_of_an_endpoint_is_checked_once_resolved() {
    let run = plan("endpoint-forms", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:13:18: `${settings.big}` must be a port number from 1 to 65535; this is 70000",
            "tay.yaml:14:32: `${settings.number}` must be text; this is a whole number",
            "tay.yaml:14:60: an endpoint's `path` starts with `/`",
            "tay.yaml:15:44: `${settings.number}` must be text; this is a whole number",
        ]
    );
}

#[test]
fn a_url_asked_of_an_endpoint_with_no_scheme_is_an_error_at_every_reference() {
    // Only the way chosen, `bare`, has no scheme, so this is found while
    // planning; the value is remembered, and each reference still says so.
    let run = plan("url-without-scheme", &[]);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:16:60: `${server.api.url}`: the endpoint has no `scheme`, so it has no URL",
            "tay.yaml:17:61: `${server.api.url}`: the endpoint has no `scheme`, so it has no URL",
        ]
    );
    assert_eq!(
        plan("url-without-scheme", &["--set", "run.default=full"]).code,
        0
    );
}
