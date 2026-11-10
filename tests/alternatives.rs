//! Units with more than one way to run, and which way each runs.

mod support;

use support::{plan, plan_json, unit};

#[test]
fn run_default_chooses_the_alternative_of_that_name() {
    let plan = plan_json("alternatives", &[]);
    let server = unit(&plan, "server");
    assert_eq!(server["alternative"], "source");
    assert_eq!(server["module"], "process");
    assert_eq!(server["runs"], "host");
}

#[test]
fn run_units_chooses_for_one_unit_over_run_default() {
    let plan = plan_json("alternatives", &["--profile", "images"]);
    let server = unit(&plan, "server");
    assert_eq!(server["alternative"], "image");
    assert_eq!(server["runs"], "own_container");
}

#[test]
fn set_chooses_over_the_profile() {
    let plan = plan_json("alternatives", &["--set", "run.units.server=image"]);
    assert_eq!(unit(&plan, "server")["alternative"], "image");
}

#[test]
fn a_unit_that_no_choice_names_an_alternative_for_is_an_error_listing_them() {
    let run = plan("alternatives", &["--profile", "neither"]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:6:3: unit `server` has the alternatives `source`, `image`, and no choice names one: `run.units` does not name it, and `run.default` is `published`, which it does not have",
            "  stack/profiles/neither.yaml:2:17: `run.default` is chosen here",
        ]
    );
}

#[test]
fn every_alternative_gives_the_same_value_names() {
    let run = plan("alternatives-names", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:12:7: alternative `image` of `server` does not give `http`, which alternative `source` gives; every alternative gives the same values",
            "  tay.yaml:11:11: `source` gives it here",
        ]
    );
}
