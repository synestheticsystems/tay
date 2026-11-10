//! Units made from templates: parameters required, defaulted, or not
//! declared, and of any YAML value.

use crate::support::{given, plan, plan_json, unit};

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
            // A mapping parameter joins with a way's `env` entry by entry: the
            // way's `REPLACED` wins and the parameter's `KEPT` is read.
            "tay.yaml:12:70: a value is text, a whole number or true or false; this is a list",
            "tay.yaml:13:51: a unit made `from` a template has only `doc`, `from`, `params`, `needs` and `after`; `uses` belongs in the template",
            // An error in the template's own text, found for each unit made
            // from it, is reported once, naming each.
            "stack/templates/bad-setting.yaml:5:11: `colour` is not a field of the settings of a `port` unit; its fields are prefer, fixed",
            "  tay.yaml:7:12: for unit `first`, made from the template here",
            "  tay.yaml:8:13: for unit `second`, made from the template here",
            // A template that uses a parameter it does not declare.
            "stack/templates/broken.yaml:6:13: `${params.nmae}` names a parameter this template does not declare",
            // A mapping cannot be put inside text.
            "stack/templates/inside-text.yaml:5:27: `${params.env}` is inside text, so its value must be text, a whole number or true or false; this is a mapping",
            "  tay.yaml:9:12: for unit `third`, made from the template here",
            // A template whose unit cannot be read is still a template, so
            // `fifth`, made from it, is not reported as made from nothing.
            "stack/templates/listed.yaml:2:7: a template's unit must be a mapping; this is a list",
            // A parameter cannot fill a field that is written out.
            "stack/templates/named.yaml:4:10: `${params.module}` cannot fill `uses`: references are not allowed in `uses`, `from`, `needs`, `after` or `action`",
            "stack/templates/nested.yaml:2:3: a template may not itself use `from`",
        ]
    );
}

#[test]
fn a_parameter_may_be_a_mapping_or_a_list_put_in_place_before_the_unit_is_read() {
    // The template's unit-wide `env` is a parameter: the mapping given joins
    // with the way's own `env`, and `command` is a list.
    let plan = plan_json("template-ok", &[]);
    let service = unit(&plan, "service");
    assert_eq!(service["way"], "source");
    assert_eq!(service["module"], "process");
}
