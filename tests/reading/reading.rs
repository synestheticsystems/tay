//! Reading definitions strictly: each error at its file, line and column, all
//! of them in one run, and nothing skipped.

use crate::support::plan;

#[test]
fn every_error_in_a_stack_is_reported_in_one_run_at_its_place() {
    let run = plan("errors-many", &[]);
    assert_eq!(run.code, 2);
    assert!(run.stdout.is_empty(), "no plan is printed");
    assert_eq!(
        run.errors(),
        [
            // An unknown key.
            "tay.yaml:6:31: `colour` is not a field of the settings of a `process` unit; its fields are command, workdir, env, secret_env, inherit, prepare",
            // A missing required field.
            "tay.yaml:9:11: `command` is required in the settings of a `process` unit",
            // A bad name, and a name references use.
            "tay.yaml:10:3: a unit's name: `Bad_Name` must start with a lower-case letter",
            "tay.yaml:11:3: `settings` cannot be a unit's name: references use it",
            "tay.yaml:12:3: `net` cannot be a unit's name: references use it",
            // A need on a unit that does not exist.
            "tay.yaml:13:30: there is no unit `nobody`",
            // A duplicate name, with the first definition.
            "stack/units/more.yaml:1:1: there is already a unit called `web`",
            "  tay.yaml:4:3: the first one is defined here",
            // A wrong type.
            "stack/units/more.yaml:4:11: `action` must be true or false; this is text",
        ]
    );
    assert!(
        run.stderr
            .ends_with("tay plan: 8 errors in the definitions; there is no plan.\n")
    );
}

#[test]
fn a_unit_file_that_does_not_parse_stops_the_plan_and_no_unit_is_skipped() {
    let run = plan("unparsable", &[]);
    assert_eq!(run.code, 2);
    assert!(run.stdout.is_empty());
    // Only the file itself is reported: the unit `one` needs `broken`, which
    // is in that file, and that is not reported as a missing unit.
    assert_eq!(
        run.errors(),
        [
            "stack/units/broken/unit.yaml:4:2: this file is not valid YAML: while parsing a block mapping, did not find expected key"
        ]
    );
}

#[test]
fn folders_and_files_that_define_no_unit_are_errors_rather_than_skipped() {
    // `.hidden.yaml`, which is not YAML, and the folder `.cache` are not
    // read at all: a name beginning with a dot is no definition.
    let run = plan("undefined-entries", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "stack/units/Bad_Unit/unit.yaml:1:1: the folder's name is the unit's name: `Bad_Unit` must start with a lower-case letter",
            "stack/units/empty.yaml:1:1: this file defines no unit: its mapping is empty",
            "stack/units/extra.yml:1:1: this is not a definition: this folder holds `<name>/unit.yaml` and `<anything>.yaml` (tay reads `.yaml`, not `.yml`)",
            "stack/units/notes:1:1: this is not a definition: this folder holds `<name>/unit.yaml` and `<anything>.yaml`, and this folder has no `unit.yaml`",
        ]
    );
}

#[test]
fn a_value_given_twice_a_run_with_no_ways_and_a_variable_set_twice_are_errors() {
    let run = plan("unit-errors", &[]);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:8:36: `name` is given twice",
            "  tay.yaml:6:14: it is given here first",
            "tay.yaml:9:17: `run` lists no ways",
            "tay.yaml:15:21: `TOKEN` is set in both `env` and `secret_env`",
            "  tay.yaml:14:14: it is set in `env` here",
        ]
    );
}

#[test]
fn a_units_own_fields_are_checked_as_written_whether_or_not_it_comes_up() {
    // Only `a` comes up; `idle` and `counted` are still read in full.
    let run = plan("idle-unit-errors", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:9:47: this must be a whole number followed by s, m or h, such as `2m`; this is `soon`",
            "tay.yaml:10:20: this must be one of `failure`, `always`, `never`; this is `sometimes`",
            "tay.yaml:10:41: this must be a whole number; this is text",
            "tay.yaml:12:20: this must be a port number; this is text",
            "tay.yaml:12:33: this must be text; this is a whole number",
            "tay.yaml:12:42: an endpoint's `path` starts with `/`",
            "tay.yaml:13:22: this must be a port number from 1 to 65535; this is 70000",
            "tay.yaml:13:35: a value is text, a whole number or true or false; this is a list",
            "tay.yaml:14:74: this must be a whole number above 0; this is -3",
            // A `doc` printed over two lines would read as part of the plan.
            "tay.yaml:17:7: `doc` is one line saying what the unit is for; this one has more",
        ]
    );
}
