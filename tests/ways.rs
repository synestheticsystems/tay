//! Units with more than one way to run, and which way each runs.

mod support;

use support::{plan, plan_json, unit};

#[test]
fn run_default_chooses_the_way_of_that_name() {
    let plan = plan_json("ways", &[]);
    let server = unit(&plan, "server");
    assert_eq!(server["way"], "source");
    assert_eq!(server["module"], "process");
    assert_eq!(server["runs"], "host");
}

#[test]
fn run_units_chooses_for_one_unit_over_run_default() {
    let plan = plan_json("ways", &["--profile", "images"]);
    let server = unit(&plan, "server");
    assert_eq!(server["way"], "image");
    assert_eq!(server["runs"], "own_container");
}

#[test]
fn set_chooses_over_the_profile() {
    let plan = plan_json("ways", &["--set", "run.units.server=image"]);
    assert_eq!(unit(&plan, "server")["way"], "image");
}

#[test]
fn a_unit_that_no_choice_names_a_way_for_is_an_error_listing_them() {
    let run = plan("ways", &["--profile", "neither"]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:6:3: unit `server` has the ways `source`, `image`, and no choice names one: `run.units` does not name it, and `run.default` (`published`) names none of them",
            "  stack/profiles/neither.yaml:2:17: `run.default` is chosen here",
        ]
    );
}

#[test]
fn every_way_gives_the_same_value_names_and_a_missing_one_is_reported_once() {
    let run = plan("ways-names", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:12:7: way `image` of `server` does not give `http`, which way `source` gives; every way gives the same values",
            "  tay.yaml:11:11: `source` gives it here",
        ]
    );
}

#[test]
fn a_way_writing_no_ready_or_restart_takes_the_ones_its_unit_writes() {
    let plan = plan_json("ways-shared", &[]);
    let server = unit(&plan, "server");
    assert_eq!(server["ready"]["check"], "log");
    assert_eq!(server["ready"]["within"]["value"], "1m");
    assert_eq!(server["restart"]["on"]["value"], "always");
    assert_eq!(server["restart"]["attempts"]["value"], 2);
    let plan = plan_json("ways-shared", &["--set", "run.units.server=image"]);
    let server = unit(&plan, "server");
    assert_eq!(server["ready"]["within"]["value"], "2m");
    assert_eq!(server["restart"]["on"]["value"], "never");
}

#[test]
fn the_with_a_unit_writes_is_joined_into_each_way_setting_by_setting_and_env_entry_by_entry() {
    // The ways replace `REPLACED`, which is not a value, and keep `KEPT`,
    // which is not one either; `source` replaces `workdir` and `image` does
    // not; and `held`, a port, has neither setting.
    let run = plan("ways-joined", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:7:7: `env` is not a field of the settings of a `port` unit; its fields are prefer, fixed",
            "tay.yaml:7:47: a value is text, a whole number or true or false; this is a list",
            "tay.yaml:8:7: `workdir` is not a field of the settings of a `port` unit; its fields are prefer, fixed",
            "tay.yaml:8:16: this must be text; this is a whole number",
        ]
    );
}

#[test]
fn names_a_way_produces_count_among_the_values_it_gives() {
    // `issued` produces `value` and `generated` gives it known before start:
    // those two agree, and only `missing` is reported.
    let run = plan("ways-produced", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:13:7: way `missing` of `token` does not give `value`, which way `issued` gives; every way gives the same values",
            "  tay.yaml:9:42: `issued` gives it here",
        ]
    );
}
