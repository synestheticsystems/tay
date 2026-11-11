//! Choices layered across the stack, the profile, this person's own file, the
//! environment's file and `--set`, later ones winning; what a choice must
//! give; and the profile, which the environment's file alone may name.

use crate::support::{fixture, given, plan_json, plan_with_home, unit, unit_names};

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
    // The profile brings up one and two, this person's own file adds three, the
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

#[test]
fn tay_yaml_gives_up_and_run_when_no_later_place_does() {
    let plan = plan_json("stack-defaults", &[]);
    assert_eq!(unit_names(&plan), ["api", "web"]);
    // `run.default` is a list: each unit takes the first of its own ways in it.
    assert_eq!(unit(&plan, "web")["way"], "image");
    assert_eq!(unit(&plan, "api")["way"], "source");
    let plan = plan_json("stack-defaults", &["--set", "run.default=source,image"]);
    assert_eq!(unit(&plan, "web")["way"], "source");
}

#[test]
fn no_place_giving_up_is_an_error() {
    let run = crate::support::plan("no-up", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:1:1: nothing says what comes up: none of tay.yaml, the profile, this person's own file, the environment's file and `--set` gives `up`"
        ]
    );
    assert_eq!(
        unit_names(&plan_json("no-up", &["--set", "up=web"])),
        ["web"]
    );
}

#[test]
fn the_label_is_the_folder_name_after_the_stack_name_unless_a_choice_sets_it() {
    let plan = plan_json("labelled-fix-login", &[]);
    let site = unit(&plan, "site");
    let value = |name: &str| given(site, name)["from_host"]["value"].clone();
    assert_eq!(value("label"), "fix-login");
    assert_eq!(value("suffix"), "-fix-login");
    assert_eq!(value("prefix"), "fix-login-");
    assert_eq!(value("host"), "portal-fix-login.example.test");
    let plan = plan_json("labelled-fix-login", &["--set", "label="]);
    let site = unit(&plan, "site");
    assert_eq!(given(site, "suffix")["from_host"]["value"], "");
    assert_eq!(
        given(site, "host")["from_host"]["value"],
        "portal.example.test"
    );
}

#[test]
fn a_required_setting_must_be_given_a_value_that_is_not_empty_by_a_choice() {
    // `base_domain` has only its default, which does not count; the
    // environment's file gives `region` an empty value.
    let run = crate::support::plan("required", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "stack/profiles/deploy.yaml:2:11: the profile `deploy` requires setting `base_domain`, and no choice gives it a value: the profile, this person's own file, the environment's file or a `--set` must (its default does not count)",
            ".tay/choices.yaml:2:11: the profile `deploy` requires setting `region`, and the value given here is empty",
            "  stack/profiles/deploy.yaml:2:37: it is required here",
        ]
    );
    let run = plan_with_home(
        "required",
        &fixture("homes/none"),
        &[
            "--set",
            "settings.base_domain=example.org",
            "--set",
            "settings.region=eu",
        ],
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
}

#[test]
fn this_persons_own_file_gives_a_required_setting_where_it_is_read() {
    // This person's own file gives `base_domain`, so only `region` is missing.
    let run = plan_with_home("required", &fixture("homes/required"), &[]);
    assert_eq!(
        run.errors(),
        [
            ".tay/choices.yaml:2:11: the profile `deploy` requires setting `region`, and the value given here is empty",
            "  stack/profiles/deploy.yaml:2:37: it is required here",
        ]
    );
}

#[test]
fn a_production_profile_counts_no_personal_file_for_a_required_setting() {
    let run = plan_with_home(
        "required",
        &fixture("homes/required"),
        &["--profile", "locked"],
    );
    assert_eq!(
        run.errors(),
        [
            "stack/profiles/locked.yaml:3:11: the profile `locked` requires setting `base_domain`, and no choice gives it a value: the profile, the environment's file or a `--set` must (its default does not count)"
        ]
    );
}

#[test]
fn production_and_require_may_be_written_only_in_a_profile() {
    let run = plan_with_home("required", &fixture("homes/profile-only"), &[]);
    assert_eq!(run.code, 2);
    let errors = run.errors();
    for field in ["production", "require"] {
        let line = errors
            .iter()
            .find(|line| line.contains(&format!("`{field}` may be written only in a profile")));
        assert!(line.is_some(), "{errors:?}");
    }
}

#[test]
fn the_environments_file_may_name_the_profile_and_the_command_line_wins() {
    assert_eq!(plan_json("profile-written", &[])["profile"], "deploy");
    let asked = plan_json("profile-written", &["--profile", "dev"]);
    assert_eq!(asked["profile"], "dev");
}

#[test]
fn a_profile_is_named_only_in_the_environments_file_and_must_exist() {
    let run = plan_with_home(
        "profile-misplaced",
        &fixture("homes/profile-misplaced"),
        &[],
    );
    assert_eq!(run.code, 2);
    let errors = run.errors();
    assert!(
        errors.contains(&".tay/choices.yaml:1:10: there is no profile `staging`; the profiles are `deploy`, `dev`"),
        "{errors:?}"
    );
    assert!(
        errors.contains(&"stack/profiles/dev.yaml:2:1: `profile` may be written only in the environment's file, `.tay/choices.yaml`"),
        "{errors:?}"
    );
    // This person's own file, which is not under the stack's root.
    let personal = errors.iter().filter(|line| {
        line.ends_with("misplaced.yaml:1:1: `profile` may be written only in the environment's file, `.tay/choices.yaml`")
    });
    assert_eq!(personal.count(), 1, "{errors:?}");
}

#[test]
fn with_more_than_one_profile_the_default_must_be_named_and_must_exist() {
    // Which profile would say what comes up is not known, so that is not
    // reported as well.
    let run = crate::support::plan("profiles-unnamed", &[]);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:1:1: there is more than one profile (`deploy`, `dev`), so `default_profile` is required"
        ]
    );
    let run = crate::support::plan("profile-default-missing", &[]);
    assert_eq!(
        run.errors(),
        ["tay.yaml:3:18: there is no profile `staging`; the profiles are `dev`"]
    );
}
