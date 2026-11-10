//! Reading definitions strictly: each error at its file, line and column, all
//! of them in one run, and nothing skipped.

mod support;

use support::plan;

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
    let run = plan("undefined-entries", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "stack/units/extra.yml:1:1: this is not a definition: this folder holds `<name>/unit.yaml` and `<anything>.yaml` (tay reads `.yaml`, not `.yml`)",
            "stack/units/notes:1:1: this is not a definition: this folder holds `<name>/unit.yaml` and `<anything>.yaml`, and this folder has no `unit.yaml`",
        ]
    );
}
