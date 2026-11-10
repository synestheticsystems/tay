//! What `tay plan` prints for a sound stack, in full, so that any change to
//! the text or to the JSON schema is noticed.

use std::fs;
use std::path::Path;

use crate::support::{fixture, plan};

/// The output with the fixture's absolute path, which differs between
/// machines, written as `<root>`.
fn printed(args: &[&str]) -> String {
    let run = plan("shop", args);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let root = fixture("shop").canonicalize().unwrap();
    run.stdout.replace(root.to_str().unwrap(), "<root>")
}

fn expected(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/expected")
            .join(name),
    )
    .unwrap()
}

#[test]
fn the_text_plan_of_the_shop_stack() {
    assert_eq!(printed(&[]), expected("shop-plan.txt"));
}

#[test]
fn the_json_plan_of_the_shop_stack_follows_schema_1() {
    let json: serde_json::Value = serde_json::from_str(&printed(&["--json"])).unwrap();
    let expected: serde_json::Value = serde_json::from_str(&expected("shop-plan.json")).unwrap();
    assert_eq!(json, expected);
}

#[test]
fn errors_in_json_have_a_place_a_message_and_the_places_involved() {
    let run = plan("errors-many", &["--json"]);
    assert_eq!(run.code, 2);
    let json = run.json();
    assert_eq!(json["schema"], 1);
    assert_eq!(json["outcome"], "errors");
    assert_eq!(
        json["errors"][6],
        serde_json::json!({
            "at": { "file": "stack/units/more.yaml", "line": 1, "column": 1 },
            "message": "there is already a unit called `web`",
            "related": [{
                "at": { "file": "tay.yaml", "line": 4, "column": 3 },
                "message": "the first one is defined here",
            }],
        })
    );
}
