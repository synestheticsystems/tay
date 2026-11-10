//! Units made from templates: parameters required, defaulted, or not declared.

mod support;

use support::{given, plan, plan_json, unit};

#[test]
fn a_default_fills_a_parameter_the_unit_does_not_give() {
    let plan = plan_json("template-ok", &[]);
    let app = unit(&plan, "app-db");
    assert_eq!(given(app, "label")["from_host"]["text"], "app owned by app");
    let report = unit(&plan, "report-db");
    assert_eq!(
        given(report, "label")["from_host"]["text"],
        "report owned by reports"
    );
    assert_eq!(given(report, "owner")["from_host"]["value"], "reports");
}

#[test]
fn a_missing_required_parameter_and_an_undeclared_one_are_errors() {
    let run = plan("template-errors", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:4:14: template `database` needs the parameter `name`, which has no default",
            "  stack/templates/database.yaml:2:3: the parameter is declared here",
            "tay.yaml:5:47: template `database` has no parameter `colour`; its parameters are name",
            "  stack/templates/database.yaml:1:1: the template is here",
            "tay.yaml:6:10: template `broken` needs the parameter `name`, which has no default",
            "  stack/templates/broken.yaml:2:3: the parameter is declared here",
            // A template that uses a parameter it does not declare.
            "stack/templates/broken.yaml:6:13: `${params.nmae}` names a parameter this template does not declare",
        ]
    );
}
