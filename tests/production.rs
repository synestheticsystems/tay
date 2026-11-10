//! A production profile refuses, naming each unit, what the format lists:
//! checked after every choice, so a choice made with `--set` is refused too.

mod support;

use support::{plan, plan_json, unit};

#[test]
fn the_same_stack_plans_in_a_profile_that_is_not_production() {
    let plan = plan_json("production", &[]);
    assert_eq!(plan["production"], false);
    let src = support::fixture("production")
        .canonicalize()
        .unwrap()
        .join("src");
    assert_eq!(
        unit(&plan, "watcher")["watch"][0]["text"],
        src.display().to_string()
    );
}

#[test]
fn each_thing_the_format_lists_is_refused_naming_its_unit() {
    let run = plan("production", &["--profile", "deploy"]);
    assert_eq!(run.code, 2);
    let marked = "  stack/profiles/deploy.yaml:1:13: the profile is marked production here";
    assert_eq!(
        run.errors(),
        [
            // A process, which runs on the host.
            "tay.yaml:8:5: the production profile `deploy` refuses unit `site`: it runs on the host",
            marked,
            // A command run on the host.
            "tay.yaml:11:5: the production profile `deploy` refuses unit `migrate`: it runs on the host",
            marked,
            // A container that builds its image.
            "tay.yaml:15:13: the production profile `deploy` refuses unit `builder`: it builds an image",
            marked,
            "tay.yaml:19:13: the production profile `deploy` refuses unit `watcher`: it watches files or folders",
            marked,
            // A mount from outside the stack's root and the shared folder that
            // does not begin with a setting's value. `inside` mounts from the
            // stack, a setting, the state folder, the shared folder and a
            // value that begins with a setting's, and `setup` runs inside a
            // container: neither is refused.
            "tay.yaml:24:24: the production profile `deploy` refuses unit `outside`: it mounts `/tmp`, which is neither under the stack's root nor in the shared folder, and does not begin with a setting's value",
            marked,
        ]
    );
}

#[test]
fn a_way_chosen_with_set_is_refused_too() {
    let clean = plan_json("production-clean", &[]);
    assert_eq!(unit(&clean, "server")["way"], "image");
    let run = plan("production-clean", &["--set", "run.units.server=source"]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:8:9: the production profile `deploy` refuses unit `server`: it runs on the host",
            "  stack/profiles/deploy.yaml:1:13: the profile is marked production here",
            "  --set run.units.server: way `source` is chosen here",
        ]
    );
}
