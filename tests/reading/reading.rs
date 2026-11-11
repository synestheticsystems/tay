//! Reading definitions strictly: each error at its file, line and column, all
//! of them in one run, and nothing skipped.

use crate::support::{fixture, given, plan, plan_with_home, unit};

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
            // The name of the folder of tay's own log.
            "tay.yaml:13:3: `tay` cannot be a unit's name: it names tay's own log",
            // A need on a unit that does not exist.
            "tay.yaml:14:30: there is no unit `nobody`",
            // A duplicate name, with the first definition.
            "stack/units/more.yaml:1:1: there is already a unit called `web`",
            "  tay.yaml:4:3: the first one is defined here",
            // A wrong type.
            "stack/units/more.yaml:4:11: `action` must be true or false; this is text",
        ]
    );
    assert!(
        run.stderr
            .ends_with("tay plan: 9 errors in the definitions; there is no plan.\n")
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
    // read at all: a name beginning with a dot is no definition. A unit
    // must be written, so a file that holds only a comment, and a unit's
    // name with nothing after it, define none.
    let run = plan("undefined-entries", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "stack/units/Bad_Unit/unit.yaml:1:1: the folder's name is the unit's name: `Bad_Unit` must start with a lower-case letter",
            "stack/units/commented.yaml:1:1: this must be a mapping, each key a unit's name; this is empty",
            "stack/units/empty.yaml:1:1: this file defines no unit: its mapping is empty",
            "stack/units/extra.yml:1:1: this is not a definition: this folder holds `<name>/unit.yaml` and `<anything>.yaml` (tay reads `.yaml`, not `.yml`)",
            "stack/units/hollow.yaml:1:7: a unit must be a mapping; this is empty",
            "stack/units/notes:1:1: this is not a definition: this folder holds `<name>/unit.yaml` and `<anything>.yaml`, and this folder has no `unit.yaml`",
        ]
    );
}

#[test]
fn nothing_written_where_every_field_is_optional_is_an_empty_mapping() {
    // The profile, the environment's file and this person's own file hold
    // only comments, and `run`, a setting's declaration, a `restart`, a
    // produced value's declaration, an action's argument and a template's
    // parameter are each a key with nothing after it.
    let home = fixture("homes/nothing-written");
    let set = ["--set", "settings.images.server=7", "--json"];
    let run = plan_with_home("nothing-written", &home, &set);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let plan = &run.json()["plan"];
    assert_eq!(plan["profile"], "dev");
    // A setting declared with nothing is text, with no default.
    let image = given(unit(plan, "maker"), "image");
    assert_eq!(image["from_host"]["value"], "7");
    let run = plan_with_home("nothing-written", &home, &[]);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:7:3: setting `images.server` has no value and no default, and the plan uses it",
            "  tay.yaml:13:20: it is used here",
            "  tay.yaml:16:22: it is used here",
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

#[test]
fn a_folder_tay_yaml_names_must_exist_though_a_default_one_may_be_absent() {
    let run = plan("folder-missing", &[]);
    let root = crate::support::fixture("folder-missing")
        .canonicalize()
        .unwrap();
    assert_eq!(
        run.errors(),
        [format!(
            "tay.yaml:3:12: the folder `{}/stack/nowhere` does not exist",
            root.display()
        )]
    );
}

#[test]
fn a_module_or_template_that_does_not_exist_is_an_error_listing_those_that_do() {
    let run = plan("unknown-names", &[]);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:5:14: `proces` is not a module; the modules are `process`, `container`, `command`, `port`, `file`, `secret`",
            "tay.yaml:6:14: there is no template `nosuch`; the templates are `db`",
        ]
    );
}

#[test]
fn ready_restart_and_watch_on_a_unit_that_is_made_are_errors_as_its_module_describes_it() {
    // `api-port`, `config` and `key` are made, as their modules say; one of
    // `site`'s ways is made, and the `restart` written for all its ways
    // would be that way's. A template's unit is read as one written out.
    let run = plan("made-runs-nothing", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:7:5: `ready` belongs to a unit that runs, and `api-port` is made: it is ready when it exists, and nothing would ever run the check",
            "tay.yaml:8:5: `restart` belongs to a unit that runs, and `api-port` is made: nothing of it runs, so nothing of it exits",
            "tay.yaml:12:5: `watch` belongs to a unit that runs, and `config` is made: nothing of it runs, so nothing would run it again",
            "tay.yaml:14:5: `restart`, written for all of `site`'s ways, belongs to a unit that runs, and a way of it is made: nothing of it runs, so nothing of it exits",
            "  tay.yaml:17:16: this way is made",
            "stack/templates/kept-secret.yaml:4:3: `ready` belongs to a unit that runs, and `key` is made: it is ready when it exists, and nothing would ever run the check",
            "  tay.yaml:18:10: for unit `key`, made from the template here",
        ]
    );
}
