//! Settings: declared, typed, set, used, and secret.

use crate::support::{given, plan, plan_json, unit};

#[test]
fn undeclared_settings_and_wrong_types_are_errors() {
    let run = plan("settings-errors", &["--profile", "dev"]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:5:20: the default is text, but `type` says the setting is a whole number",
            // Used but not declared.
            "tay.yaml:9:21: `${settings.size}`: no setting `size` is declared in tay.yaml",
            // Set with the wrong type, pointing at the declaration.
            "stack/profiles/dev.yaml:2:10: setting `count` is a whole number; this value is text",
            "  tay.yaml:4:3: the setting is declared here",
            // Set but not declared.
            "stack/profiles/dev.yaml:3:3: no setting `colour` is declared in tay.yaml, so it cannot be set",
        ]
    );
}

#[test]
fn a_setting_with_no_value_is_an_error_only_where_the_plan_uses_it_naming_every_use() {
    // `images.unused` has no value either, but only a unit outside the plan uses it.
    let run = plan("settings-unset", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:4:3: setting `region` has no value and no default, and the plan uses it",
            "  tay.yaml:12:15: it is used here",
            "  tay.yaml:13:18: it is used here",
        ]
    );
}

#[test]
fn set_gives_a_value_the_type_of_its_setting() {
    let plan = plan_json(
        "settings-unset",
        &["--set", "settings.region=eu", "--set", "settings.count=7"],
    );
    let one = unit(&plan, "one");
    assert_eq!(given(one, "where")["from_host"]["value"], "eu");
    assert_eq!(given(one, "count")["from_host"]["value"], 7);
}

#[test]
fn a_set_value_that_does_not_fit_its_type_is_an_error_that_does_not_quote_it() {
    let run = plan(
        "settings-unset",
        &[
            "--set",
            "settings.region=eu",
            "--set",
            "settings.count=abc",
            "--set",
            "settings.pin=hunter2",
        ],
    );
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "--set settings.count: setting `count` is a whole number, and this value is not",
            "  tay.yaml:6:3: the setting is declared here",
            "--set settings.pin: setting `pin` is a whole number, and this value is not",
            "  tay.yaml:7:3: the setting is declared here",
        ]
    );
    assert!(!run.stderr.contains("hunter2"));
}

#[test]
fn a_secret_setting_is_shown_as_secret() {
    let plan = plan_json("settings-unset", &["--set", "settings.region=eu"]);
    let pin = given(unit(&plan, "one"), "pin");
    assert_eq!(pin["secret"], true);
    assert_eq!(pin["from_host"]["text"], "<secret>");
    assert_eq!(pin["from_host"]["value"], serde_json::Value::Null);
}
