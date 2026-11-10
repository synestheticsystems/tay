//! Actions: units that run only when asked for by name, and the arguments
//! they may be asked with.

mod support;

use support::{given, plan, plan_json, unit};

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
