//! A tay too old for the definitions stops with the version it would need,
//! before reading anything it might not know.

use crate::support::plan;

#[test]
fn definitions_that_require_a_newer_tay_exit_3_with_the_version_needed() {
    // tay.yaml also has a field this tay does not know; it is not reported,
    // because a newer format may have it.
    let run = plan("too-new", &[]);
    assert_eq!(run.code, 3);
    assert_eq!(
        run.errors(),
        [format!(
            "tay.yaml:2:11: these definitions need tay 99.0.0 or newer; this is tay {}",
            env!("CARGO_PKG_VERSION")
        )]
    );
}

#[test]
fn an_unknown_format_exits_3() {
    let run = plan("format-two", &[]);
    assert_eq!(run.code, 3);
    assert_eq!(
        run.errors(),
        [format!(
            "tay.yaml:1:9: these definitions are written in format 2, and this tay ({}) reads format 1 only; a tay that reads format 2 is needed",
            env!("CARGO_PKG_VERSION")
        )]
    );
}

#[test]
fn json_says_too_old_with_the_same_reason() {
    let run = plan("too-new", &["--json"]);
    assert_eq!(run.code, 3);
    let json = run.json();
    assert_eq!(json["outcome"], "too_old");
    assert_eq!(
        json["errors"][0]["at"],
        serde_json::json!({ "file": "tay.yaml", "line": 2, "column": 11 })
    );
}
