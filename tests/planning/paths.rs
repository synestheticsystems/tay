//! Paths: one that begins `./` is relative to the folder of the file it is
//! written in, any other relative one to the stack's root, and the value a
//! unit gives for one is the whole path.

use crate::support::{fixture, given, plan, plan_json, unit};
use serde_json::json;

fn root() -> String {
    fixture("paths")
        .canonicalize()
        .unwrap()
        .display()
        .to_string()
}

#[test]
fn an_output_is_its_whole_path_and_using_it_is_still_a_need() {
    let plan = plan_json("paths", &[]);
    let server = given(unit(&plan, "web"), "server");
    assert_eq!(
        server["from_host"]["text"],
        format!("{}/target/debug/server", root())
    );
    assert_eq!(server["known_before_start"], false);
    assert_eq!(
        unit(&plan, "web")["needs"][0],
        json!({ "unit": "build", "because": "uses_produced_value", "value": "build.outputs.server" })
    );
}

#[test]
fn a_path_beginning_dot_slash_is_relative_to_its_file_and_any_other_to_the_root() {
    let plan = plan_json("paths", &[]);
    let watched: Vec<&str> = unit(&plan, "web")["watch"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| path["text"].as_str().unwrap())
        .collect();
    let root = root();
    assert_eq!(
        watched,
        [
            format!("{root}/stack/units/web/unit.yaml"),
            format!("{root}/src")
        ]
    );
    // `conf` reads `./site.conf.tmpl` from its own folder; read from the
    // root, it would not be found.
    assert_eq!(unit(&plan, "conf")["made"], true);
}

#[test]
fn a_mount_source_must_exist_unless_in_the_state_log_or_shared_folder_or_produced() {
    // `box` mounts from the stack, the state folder, the log folder, the
    // shared folder and a built file: none is an error.
    assert_eq!(plan("paths", &[]).code, 0);
    let run = plan("paths", &["--set", "up_add=lost"]);
    assert_eq!(
        run.errors(),
        [format!(
            "tay.yaml:26:24: the mount's source `{}/missing` does not exist; outside the state, log and shared folders it must exist when tay plans",
            root()
        )]
    );
}

#[test]
fn a_file_name_and_template_path_built_from_values_known_before_start_are_checked_once_resolved() {
    let plan = plan_json("paths", &[]);
    assert_eq!(
        given(unit(&plan, "named-conf"), "path")["from_host"]["value"],
        format!("{}/.tay/state/files/named-conf/site.conf", root())
    );
}

#[test]
fn a_watched_path_must_exist() {
    let run = plan("paths", &["--set", "up=forgetful"]);
    assert_eq!(
        run.errors(),
        [format!(
            "tay.yaml:35:13: `{}/nothing-here`, which this unit watches, does not exist",
            root()
        )]
    );
}

#[test]
fn a_dot_slash_path_in_a_settings_value_is_relative_to_where_the_value_is_written() {
    // `settled`, in its own folder, mounts `${settings.config_folder}`,
    // whose default `./config` is written in tay.yaml: the root's `config`.
    assert_eq!(plan("paths", &["--set", "up=settled"]).code, 0);
    // A value given with `--set` is relative to the stack's root.
    let run = plan(
        "paths",
        &[
            "--set",
            "up=settled",
            "--set",
            "settings.config_folder=./elsewhere",
        ],
    );
    assert_eq!(
        run.errors(),
        [format!(
            "stack/units/settled/unit.yaml:4:20: the mount's source `{}/elsewhere` does not exist; outside the state, log and shared folders it must exist when tay plans",
            root()
        )]
    );
}

#[test]
fn a_dot_slash_path_a_templates_parameter_brings_is_relative_to_the_unit_that_writes_it() {
    // The template puts `dir` in whole, and at the start of text: either
    // way `./data` is the folder of the unit that gives it.
    let plan = plan_json("template-paths", &[]);
    let data = fixture("template-paths")
        .canonicalize()
        .unwrap()
        .join("stack/units/watcher/data");
    let watched: Vec<&str> = unit(&plan, "watcher")["watch"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| path["value"].as_str().unwrap())
        .collect();
    assert_eq!(
        watched,
        [data.display().to_string(), format!("{}/.", data.display())]
    );
}
