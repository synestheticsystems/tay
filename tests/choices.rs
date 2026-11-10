//! Choices layered across the stack, the profile, the personal file, the
//! environment's file and `--set`, later ones winning.

mod support;

use support::{fixture, given, plan_with_home, unit, unit_names};

fn plan(args: &[&str]) -> serde_json::Value {
    let mut all = args.to_vec();
    all.push("--json");
    let run = plan_with_home("layers", &fixture("homes/layers"), &all);
    assert_eq!(run.code, 0, "{}", run.stdout);
    run.json()["plan"].clone()
}

fn setting_from(plan: &serde_json::Value, place: &str) -> String {
    given(unit(plan, "one"), place)["from_host"]["value"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn each_place_wins_over_the_ones_before_it() {
    let plan = plan(&["--set", "settings.from.argument=argument"]);
    assert_eq!(setting_from(&plan, "stack"), "stack");
    assert_eq!(setting_from(&plan, "profile"), "profile");
    assert_eq!(setting_from(&plan, "personal"), "personal");
    assert_eq!(setting_from(&plan, "environment"), "environment");
    assert_eq!(setting_from(&plan, "argument"), "argument");
}

#[test]
fn up_from_the_profile_is_adjusted_by_up_add_and_up_remove_in_later_places() {
    // The profile brings up one and two, the personal file adds three, the
    // environment's file removes two, and `--set` adds four.
    let plan = plan(&["--set", "up_add=four"]);
    let mut names = unit_names(&plan);
    names.sort_unstable();
    assert_eq!(names, ["four", "one", "three"]);
}

#[test]
fn a_later_up_replaces_the_list() {
    assert_eq!(unit_names(&plan(&["--set", "up=two"])), ["two"]);
}

#[test]
fn a_production_profile_does_not_read_the_personal_file() {
    let plan = plan(&["--profile", "prod"]);
    assert_eq!(plan["production"], true);
    assert_eq!(setting_from(&plan, "personal"), "profile");
    assert_eq!(setting_from(&plan, "environment"), "environment");
    assert_eq!(unit_names(&plan), ["one"]);
}
