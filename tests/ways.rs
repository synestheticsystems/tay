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
