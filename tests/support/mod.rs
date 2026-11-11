//! Running the `tay` binary against the fixture stacks in `tests/fixtures`.

// Each test binary uses its own share of these helpers.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// How `tay plan` prints `${host.listen}` in a plan in which something runs
/// in a container. Where Docker runs in a virtual machine of its own, as on
/// a Mac, the address is known before anything starts; where it runs on the
/// host itself, as on Linux, it is the host's address on the environment's
/// network, which is made only as the environment is brought up.
pub const PLANNED_HOST_LISTEN: &str = if cfg!(target_os = "macos") {
    "127.0.0.1"
} else {
    "<host.listen>"
};

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    /// What tay printed about errors, without its closing count: each error's
    /// line, and the indented lines of the places it involves.
    pub fn errors(&self) -> Vec<&str> {
        self.stderr
            .lines()
            .filter(|line| !line.starts_with("tay plan: "))
            .collect()
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|error| panic!("tay printed JSON ({error}): {}", self.stdout))
    }

    /// Each line of standard output that is a change of state, as
    /// `unit state detail` with the spacing taken out.
    pub fn changes(&self) -> Vec<String> {
        self.stdout
            .lines()
            .filter(|line| line.starts_with("  "))
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    }

    /// Standard output before its last line, which, of a change that
    /// brings units up, says how long it took.
    pub fn before_it_took(&self) -> &str {
        let last = (self.stdout.trim_end_matches('\n').rfind('\n')).map_or(0, |at| at + 1);
        assert!(
            self.stdout[last..].contains(": it took "),
            "the last line says how long it took:\n{}",
            self.stdout
        );
        &self.stdout[..last]
    }

    /// The last change of state `unit` printed, as `state detail`.
    pub fn last_change(&self, unit: &str) -> String {
        self.changes()
            .into_iter()
            .filter_map(|line| line.strip_prefix(&format!("{unit} ")).map(str::to_string))
            .next_back()
            .unwrap_or_else(|| panic!("`{unit}` changed state:\n{}", self.stdout))
    }
}

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A home folder with no personal choices in it.
fn no_home() -> PathBuf {
    fixture("homes/none")
}

/// Runs tay in `dir` with `home` as the home folder.
pub fn tay(dir: &Path, home: &Path, args: &[&str]) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_tay"))
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .output()
        .expect("the tay binary runs");
    Run {
        code: output.status.code().expect("tay exits with a code"),
        stdout: String::from_utf8(output.stdout).expect("output is UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("errors are UTF-8"),
    }
}

/// `tay plan` on the fixture stack `name`.
pub fn plan(name: &str, args: &[&str]) -> Run {
    plan_with_home(name, &no_home(), args)
}

pub fn plan_with_home(name: &str, home: &Path, args: &[&str]) -> Run {
    let root = fixture(name);
    let mut all = vec![
        "plan",
        "--root",
        root.to_str().expect("fixture paths are UTF-8"),
    ];
    all.extend(args);
    tay(Path::new(env!("CARGO_MANIFEST_DIR")), home, &all)
}

/// The plan `tay plan --json` prints for a sound stack.
pub fn plan_json(name: &str, args: &[&str]) -> Value {
    let mut all = args.to_vec();
    all.push("--json");
    let run = plan(name, &all);
    assert_eq!(run.code, 0, "the plan should be sound: {}", run.stdout);
    run.json()["plan"].clone()
}

pub fn unit<'p>(plan: &'p Value, name: &str) -> &'p Value {
    plan["units"]
        .as_array()
        .expect("units is a list")
        .iter()
        .find(|unit| unit["name"] == name)
        .unwrap_or_else(|| panic!("unit `{name}` is in the plan"))
}

pub fn unit_names(plan: &Value) -> Vec<&str> {
    plan["units"]
        .as_array()
        .expect("units is a list")
        .iter()
        .map(|unit| unit["name"].as_str().expect("a unit has a name"))
        .collect()
}

/// The value a unit gives, as `{ "text", "value" }` from the host.
pub fn given<'p>(unit: &'p Value, name: &str) -> &'p Value {
    unit["gives"]
        .as_array()
        .expect("gives is a list")
        .iter()
        .find(|given| given["name"] == name)
        .unwrap_or_else(|| panic!("`{name}` is given"))
}
