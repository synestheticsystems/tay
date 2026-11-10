//! The command line: finding the stack, the verbs that exist, and that
//! planning writes nothing.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use support::{fixture, plan, tay};

fn home() -> PathBuf {
    fixture("homes/none")
}

#[test]
fn version_and_help_succeed() {
    let run = tay(&fixture("shop"), &home(), &["--version"]);
    assert_eq!(
        (run.code, run.stdout.as_str()),
        (0, concat!("tay ", env!("CARGO_PKG_VERSION"), "\n"))
    );
    let run = tay(&fixture("shop"), &home(), &["--help"]);
    assert_eq!(run.code, 0);
    assert!(
        run.stdout
            .contains("tay plan [--root FOLDER] [--profile NAME] [--set PLACE=VALUE]... [--json]")
    );
}

#[test]
fn any_other_verb_says_it_is_not_built_and_exits_2() {
    let run = tay(&fixture("shop"), &home(), &["up"]);
    assert_eq!(run.code, 2);
    assert!(run.stdout.is_empty());
    assert!(
        run.stderr.starts_with("tay up: not built yet."),
        "{}",
        run.stderr
    );
}

#[test]
fn an_unknown_option_of_plan_exits_2() {
    let run = tay(&fixture("shop"), &home(), &["plan", "--colour"]);
    assert_eq!(run.code, 2);
    assert!(
        run.stderr
            .starts_with("tay plan: `--colour` is not an option of `tay plan`")
    );
}

#[test]
fn the_stack_is_found_by_walking_up_from_the_current_folder() {
    let run = tay(
        &fixture("shop/stack/units/web"),
        &home(),
        &["plan", "--json"],
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["plan"]["stack"], "shop");
    let run = tay(
        &fixture("homes"),
        &home(),
        &["plan", "--root", "../shop/stack"],
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
}

#[test]
fn a_folder_with_no_stack_above_it_exits_2() {
    let empty = std::env::temp_dir();
    let run = tay(&empty, &home(), &["plan"]);
    assert_eq!(run.code, 2);
    assert!(
        run.stderr.contains("there is no tay.yaml in"),
        "{}",
        run.stderr
    );
}

#[test]
fn an_unknown_profile_is_an_error() {
    let run = plan("shop", &["--profile", "staging"]);
    assert_eq!(run.code, 2);
    assert_eq!(
        run.errors(),
        ["--profile staging: there is no profile `staging`; the profiles are `deploy`, `dev`"]
    );
}

/// Every file under `folder`, with its size and when it last changed.
fn snapshot(folder: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    let mut files = BTreeMap::new();
    let mut folders = vec![folder.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                folders.push(entry.path());
            }
            files.insert(entry.path(), (meta.len(), meta.modified().unwrap()));
        }
    }
    files
}

#[test]
fn planning_writes_nothing_even_where_the_environment_has_its_folder() {
    // This stack has a `.tay` folder and a file unit, whose file tay must not write.
    let root = fixture("secrets-allowed");
    let before = snapshot(&root);
    assert_eq!(plan("secrets-allowed", &[]).code, 0);
    assert_eq!(plan("secrets-allowed", &["--json"]).code, 0);
    assert_eq!(snapshot(&root), before);
}
