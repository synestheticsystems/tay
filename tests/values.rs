//! Values and references: types kept or made text, values known before start
//! against values produced by running, and circles.

mod support;

use serde_json::json;
use support::{given, plan, plan_json, unit, unit_names};

#[test]
fn a_whole_string_reference_keeps_its_type_and_one_inside_text_becomes_text() {
    let plan = plan_json("value-types", &[]);
    let port = unit(&plan, "fixed-port");
    assert_eq!(given(port, "whole")["from_host"]["value"], json!(8080));
    assert_eq!(
        given(port, "inside")["from_host"]["value"],
        json!("port 8080")
    );
    assert_eq!(given(port, "flag")["from_host"]["value"], json!(true));
}

#[test]
fn a_doubled_dollar_writes_a_literal_reference_opening() {
    let plan = plan_json("value-types", &[]);
    let escaped = given(unit(&plan, "fixed-port"), "escaped");
    assert_eq!(
        escaped["from_host"]["value"],
        json!("${settings.port} stays as written")
    );
}

#[test]
fn a_setting_is_literal_so_a_reference_opening_in_its_value_stays_as_written() {
    let plan = plan_json("value-types", &[]);
    let pattern = given(unit(&plan, "fixed-port"), "pattern");
    assert_eq!(
        pattern["from_host"]["value"],
        json!("${not.a.reference} and $${this}")
    );
    let plan = plan_json("value-types", &["--set", "settings.pattern=${set.too}"]);
    let pattern = given(unit(&plan, "fixed-port"), "pattern");
    assert_eq!(pattern["from_host"]["value"], json!("${set.too}"));
}

#[test]
fn references_to_the_environment_and_the_host() {
    let plan = plan_json("value-types", &[]);
    let port = unit(&plan, "fixed-port");
    let text = |name: &str| given(port, name)["from_host"]["text"].clone();
    assert_eq!(text("name"), "value-types");
    // The folder is named after the stack, so the label is empty.
    assert_eq!(text("label"), "");
    assert_eq!(text("profile"), "dev");
    let root = support::fixture("value-types").canonicalize().unwrap();
    assert_eq!(text("state"), format!("{}/.tay/state", root.display()));
    assert_eq!(text("logs"), format!("{}/.tay/logs", root.display()));
    let home = support::fixture("homes/none");
    assert_eq!(
        text("shared"),
        format!("{}/.local/share/tay/value-types", home.display())
    );
    // None of these is known until the environment is first brought up.
    assert_eq!(text("id"), "<environment.id>");
    assert_eq!(text("network"), "<environment.network>");
    assert_eq!(text("listen"), "<host.listen>");
}

#[test]
fn a_whole_string_reference_of_the_wrong_type_is_an_error() {
    // The setting is the text "8080", and a port number is wanted.
    let run = plan("value-type-error", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        ["tay.yaml:8:20: `${settings.port}` must be a port number; this is text"]
    );
}

#[test]
fn using_an_address_orders_nothing_so_two_units_can_use_each_others_and_plan() {
    // `login`'s settings use `portal`'s address while `portal` needs `login`:
    // the address is known before start, so there is no circle.
    let planned = plan_json("known-and-produced", &[]);
    let names = unit_names(&planned);
    let at = |name: &str| names.iter().position(|n| *n == name).unwrap();
    assert!(at("login") < at("portal"));
    assert_eq!(
        unit(&planned, "portal")["needs"],
        json!([
            { "unit": "portal-port", "because": "written" },
            { "unit": "login", "because": "written" },
            { "unit": "login-port", "because": "uses_made_value", "value": "login-port.number" },
        ])
    );
}

#[test]
fn a_value_produced_by_running_adds_a_need_and_the_plan_says_why() {
    let planned = plan_json("known-and-produced", &[]);
    assert_eq!(
        unit(&planned, "login")["needs"],
        json!([
            { "unit": "login-port", "because": "written" },
            { "unit": "portal-port", "because": "uses_made_value", "value": "portal-port.number" },
            { "unit": "issuer", "because": "uses_produced_value", "value": "issuer.token" },
        ])
    );
    let token = given(unit(&planned, "issuer"), "token");
    assert_eq!(token["known_before_start"], false);
    assert_eq!(token["from_host"]["text"], "<produced: issuer.token>");
    let text = plan("known-and-produced", &[]).stdout;
    assert!(text.contains(
        "   needs issuer, because it uses issuer.token, which issuer produces by running\n"
    ));
}

#[test]
fn a_circle_among_values_known_before_start_is_an_error_showing_the_circle() {
    let run = plan("value-circle", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:6:27: a circle among values known before start: `first.url`, which is built from `second.url`, which is built from `first.url`",
            "  tay.yaml:9:20: the circle closes here",
        ]
    );
}

#[test]
fn a_circle_of_needs_is_an_error_showing_the_whole_circle() {
    let run = plan("needs-circle", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:4:28: a circle among units: `a` needs `b`, `b` needs `c`, and `c` needs `a`",
            "  tay.yaml:6:28: the circle closes here",
        ]
    );
}

#[test]
fn after_brings_nothing_in_and_an_action_comes_up_only_when_named() {
    let dev = plan_json("actions", &[]);
    assert_eq!(unit_names(&dev), ["app"]);
    assert_eq!(
        unit(&dev, "app")["after"],
        json!([{ "unit": "logger", "comes_up": false }])
    );
    let seeding = plan_json("actions", &["--profile", "seeding"]);
    assert_eq!(unit_names(&seeding), ["app", "seed"]);
    let run = plan("actions", &["--profile", "broken"]);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:13:37: unit `needs-seed` needs `seed`, which is an action: it comes up only when `up` names it"
        ]
    );
}
