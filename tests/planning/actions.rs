//! Actions: units that run only when asked for by name, and the arguments
//! they may be asked with.

use crate::support::{given, plan, plan_json, unit, unit_names};
use serde_json::json;

#[test]
fn an_actions_arguments_print_as_placeholders() {
    let plan = plan_json("actions", &["--set", "up=mint"]);
    let mint = unit(&plan, "mint");
    assert_eq!(given(mint, "user")["from_host"]["text"], "<arg: user>");
    assert_eq!(given(mint, "hours")["from_host"]["text"], "<arg: hours>");
    // An argument is known before start: the action is asked with it.
    assert_eq!(given(mint, "user")["known_before_start"], true);
}

#[test]
fn args_go_only_with_an_action_and_a_reference_names_one_it_declares() {
    let run = plan("action-errors", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:7:5: `args` goes with `action: true`: only an action is asked for with arguments",
            "tay.yaml:10:18: `${args.x}` is an action's argument, and unit `other` is not an action",
            "tay.yaml:15:26: `${args.nobody}`: action `mint` declares no argument `nobody`",
        ]
    );
}

#[test]
fn after_brings_nothing_in_and_an_action_comes_up_only_when_named() {
    let dev = plan_json("actions", &[]);
    assert_eq!(unit_names(&dev), ["app"]);
    assert_eq!(
        unit(&dev, "app")["after"],
        json!([{ "unit": "logger", "comes_up": false }])
    );
    let seeding = plan_json("actions", &["--profile", "seeding"]);
    assert_eq!(unit_names(&seeding), ["app", "seed"]);
    let run = plan("actions", &["--profile", "broken"]);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:13:37: unit `needs-seed` needs `seed`, which is an action: it comes up only when `up` names it"
        ]
    );
}
