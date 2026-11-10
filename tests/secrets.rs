//! Secrets: where they may go, and that nothing secret is ever printed.

mod support;

use support::{given, plan, plan_json, unit};

/// The value of the secret setting, in the fixture's `.tay/choices.yaml`.
const LICENCE: &str = "LICENCE-VALUE-0451";

#[test]
fn a_secret_may_go_in_secret_env_a_file_and_a_value_given() {
    let plan = plan_json("secrets-allowed", &[]);
    let database = unit(&plan, "database");
    let dsn = given(database, "dsn");
    assert_eq!(dsn["secret"], true);
    assert_eq!(
        dsn["from_host"]["text"],
        "postgres://app:<secret>@database/app"
    );
    assert_eq!(dsn["from_host"]["value"], serde_json::Value::Null);
    assert_eq!(
        given(database, "licence")["from_containers"]["text"],
        "<secret>"
    );
}

#[test]
fn nothing_secret_is_printed_in_text_or_json() {
    let text = plan("secrets-allowed", &[]);
    let json = plan("secrets-allowed", &["--json"]);
    assert_eq!((text.code, json.code), (0, 0));
    for output in [&text.stdout, &text.stderr, &json.stdout, &json.stderr] {
        assert!(
            !output.contains(LICENCE),
            "the secret setting's value was printed"
        );
    }
    assert!(text.stdout.contains("gives licence = <secret> (secret)"));
}

#[test]
fn a_literal_in_secret_env_is_an_error() {
    let run = plan("secrets-literal", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:8:28: `secret_env` takes only a reference to a secret value; a secret written into a definition is not a secret"
        ]
    );
    assert!(!run.stderr.contains("hunter2-literal"));
}

#[test]
fn a_secret_where_other_programs_can_read_it_is_an_error() {
    let run = plan("secrets-forbidden", &[]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        [
            "tay.yaml:9:22: a secret (`db-password.value`) cannot go in a command line: other programs can read it",
            "tay.yaml:10:24: a secret (`db-password.value`) cannot go in an environment variable in `env` (a secret goes in `secret_env`): other programs can read it",
            "tay.yaml:11:27: `secret_env` takes only references to secret values, and this value is not secret",
            "tay.yaml:16:27: a secret (`db-password.value`) cannot go in a label: other programs can read it",
            "tay.yaml:17:31: a secret (`db-password.value`) cannot go in a readiness check: other programs can read it",
            "tay.yaml:20:22: a secret (`db-password.value`) cannot go in a file whose mode is not \"0600\": other programs can read it",
        ]
    );
}
