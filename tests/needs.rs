//! What a unit needs and why: a need it writes, the container a command runs
//! inside, and a value it uses of a unit that is made; and the order that
//! follows.

mod support;

use serde_json::json;
use support::{plan, plan_json, unit, unit_names};

#[test]
fn using_a_value_of_a_unit_that_is_made_brings_it_in_and_is_a_need() {
    let plan = plan_json("made-values", &[]);
    // `web` uses `api`'s address, which is built from `api-port`'s number:
    // `api-port` comes up and is a need, and `api` itself does not come up.
    assert_eq!(
        unit(&plan, "web")["needs"],
        json!([
            { "unit": "web-port", "because": "uses_made_value", "value": "web-port.number" },
            { "unit": "api-port", "because": "uses_made_value", "value": "api-port.number" },
            { "unit": "zz-config", "because": "uses_made_value", "value": "zz-config.path" },
            { "unit": "token-file", "because": "uses_made_value", "value": "token-file.path" },
        ])
    );
    let names = unit_names(&plan);
    assert!(!names.contains(&"api"), "{names:?}");
    assert!(!names.contains(&"unused-port"), "{names:?}");
    assert_eq!(unit(&plan, "web-port")["made"], true);
    assert_eq!(unit(&plan, "web-port")["runs"], json!(null));
    let text = plan_text();
    assert!(text.contains("   needs api-port, because it uses api-port.number\n"));
    assert!(text.contains("web-port: uses `port`, is made\n"));
}

#[test]
fn a_unit_made_only_from_values_known_before_start_is_made_before_anything_starts() {
    // `a-worker` runs and needs nothing, so only being made puts the files
    // and ports before it; `token-file` is built from a value `issuer`
    // produces, so it comes after `issuer`.
    let plan = plan_json("made-values", &[]);
    assert_eq!(
        unit_names(&plan),
        [
            "api-port",
            "web-port",
            "zz-config",
            "a-worker",
            "issuer",
            "token-file",
            "web"
        ]
    );
}

#[test]
fn a_commands_in_is_a_need_and_is_not_written_again() {
    let run = plan("in-written-again", &[]);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:6:35: `db` is a need already, through `in`; it is not written again in `needs`",
            "  tay.yaml:6:52: `in` names it here",
        ]
    );
}

#[test]
fn the_container_a_command_runs_inside_comes_up_as_its_need() {
    let plan = plan_json("in-needs", &[]);
    assert_eq!(unit_names(&plan), ["db", "setup"]);
    assert_eq!(
        unit(&plan, "setup")["needs"],
        json!([{ "unit": "db", "because": "in" }])
    );
}

fn plan_text() -> String {
    let run = plan("made-values", &[]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    run.stdout
}
