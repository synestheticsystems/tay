//! The command line: `tay plan`, `tay --version` and `tay --help`.

mod output;
mod print;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::definition::read::{self, STACK_FILE, THIS_TAY};
use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Sources;
use crate::plan::{self, Options, Outcome};
use output::{json, text};
use print::{err, out};

/// The plan is sound.
const SOUND: u8 = 0;
/// The definitions, or the command line, have errors.
const ERRORS: u8 = 2;
/// This tay is too old for the definitions.
const TOO_OLD: u8 = 3;

const HELP: &str = "\
tay (Topology Automation via YAML) brings a stack of host processes and
containers up in dependency order and reports their state. This version reads
a stack's definitions and plans; it starts nothing.

Usage:
  tay plan [--root FOLDER] [--profile NAME] [--set PLACE=VALUE]... [--json]
  tay --version
  tay --help

tay plan reads the stack whose tay.yaml is in the current folder or the
nearest folder above it, applies the choices, and prints each unit that would
come up, in an order it could start in: the module it uses and where it runs,
what it needs and why, how it is known to be ready, the host ports it opens
and what opens each, and the values it gives. Of the environment's .tay/
folder it reads choices.yaml and nothing else, and it writes nothing.

Options of tay plan:
  --root FOLDER       look for tay.yaml from FOLDER instead of the current one
  --profile NAME      use this profile instead of the stack's default one
  --set PLACE=VALUE   make one choice, after every other place. PLACE is up,
                      up_add, up_remove, run.default, run.units.UNIT,
                      settings.NAME or label; a list is written with commas
  --json              print the plan as JSON, schema 1, for programs

Exit codes: 0 the plan is sound; 2 the definitions or the command line have
errors; 3 this tay is too old for the definitions.
";

pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let mut args = args.into_iter();
    let Some(verb) = args.next() else {
        err(HELP);
        return ExitCode::from(ERRORS);
    };
    match verb.to_str() {
        Some("--help" | "-h" | "help") => {
            out(HELP);
            ExitCode::from(SOUND)
        }
        Some("--version" | "-V") => {
            out(&format!("tay {THIS_TAY}\n"));
            ExitCode::from(SOUND)
        }
        Some("plan") => match PlanArgs::parse(args) {
            Ok(Some(plan_args)) => plan_args.run(),
            Ok(None) => {
                out(HELP);
                ExitCode::from(SOUND)
            }
            Err(message) => {
                err(&format!(
                    "tay plan: {message}\nRun `tay --help` for how to use it.\n"
                ));
                ExitCode::from(ERRORS)
            }
        },
        Some(other) if other.starts_with('-') => {
            err(&format!(
                "tay: unknown option `{other}`\nRun `tay --help` for how to use it.\n"
            ));
            ExitCode::from(ERRORS)
        }
        _ => {
            err(&format!(
                "tay {}: not built yet. This version of tay reads a stack's definitions and plans it (`tay plan`); it starts nothing.\n",
                verb.to_string_lossy()
            ));
            ExitCode::from(ERRORS)
        }
    }
}

struct PlanArgs {
    root: Option<PathBuf>,
    profile: Option<String>,
    sets: Vec<String>,
    json: bool,
}

impl PlanArgs {
    /// The arguments after `plan`; `None` when help is asked for.
    fn parse(args: impl Iterator<Item = OsString>) -> Result<Option<PlanArgs>, String> {
        let mut parsed = PlanArgs {
            root: None,
            profile: None,
            sets: Vec::new(),
            json: false,
        };
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            let arg_text = arg.to_string_lossy().into_owned();
            let (flag, inline) = match arg_text.split_once('=') {
                Some((flag, value)) if flag.starts_with("--") => {
                    (flag.to_string(), Some(value.to_string()))
                }
                _ => (arg_text.clone(), None),
            };
            let mut value = |name: &str| -> Result<OsString, String> {
                match &inline {
                    Some(value) => Ok(OsString::from(value)),
                    None => args.next().ok_or_else(|| format!("`{name}` needs a value")),
                }
            };
            let text = |value: OsString, name: &str| {
                value
                    .into_string()
                    .map_err(|_| format!("the value of `{name}` is not valid UTF-8"))
            };
            match flag.as_str() {
                "--help" | "-h" => return Ok(None),
                "--json" if inline.is_none() => parsed.json = true,
                "--root" => parsed.root = Some(PathBuf::from(value("--root")?)),
                "--profile" => parsed.profile = Some(text(value("--profile")?, "--profile")?),
                "--set" => parsed.sets.push(text(value("--set")?, "--set")?),
                _ => return Err(format!("`{arg_text}` is not an option of `tay plan`")),
            }
        }
        Ok(Some(parsed))
    }

    fn run(self) -> ExitCode {
        let start = match self.root.clone().map_or_else(std::env::current_dir, Ok) {
            Ok(start) => start,
            Err(error) => return self.fail(&format!("the current folder cannot be read: {error}")),
        };
        let start = match start.canonicalize() {
            Ok(start) => start,
            Err(error) => {
                return self.fail(&format!(
                    "the folder `{}` cannot be used: {error}",
                    start.display()
                ));
            }
        };
        let Some(root) = read::find_root(&start) else {
            return self.fail(&format!(
                "there is no {STACK_FILE} in {} or any folder above it",
                start.display()
            ));
        };
        let options = Options {
            profile: self.profile.clone(),
            sets: self.sets.clone(),
            home: std::env::var_os("HOME").map(PathBuf::from),
        };
        let mut sources = Sources::default();
        let mut d = Diagnostics::default();
        match plan::plan(&root, &options, &mut sources, &mut d) {
            Outcome::Plan(plan) => {
                if self.json {
                    print_json(&json::plan(&plan));
                } else {
                    out(&text::plan(&plan));
                }
                ExitCode::from(SOUND)
            }
            Outcome::Errors => {
                let errors = d.into_sorted();
                if self.json {
                    print_json(&json::errors("errors", &errors, &sources));
                } else {
                    for error in &errors {
                        err(&format!("{}\n", error.render(&sources)));
                    }
                    let count = match errors.len() {
                        1 => "1 error".to_string(),
                        n => format!("{n} errors"),
                    };
                    err(&format!(
                        "tay plan: {count} in the definitions; there is no plan.\n"
                    ));
                }
                ExitCode::from(ERRORS)
            }
            Outcome::TooOld(reasons) => {
                if self.json {
                    print_json(&json::errors("too_old", &reasons, &sources));
                } else {
                    for reason in &reasons {
                        err(&format!("{}\n", reason.render(&sources)));
                    }
                }
                ExitCode::from(TOO_OLD)
            }
        }
    }

    fn fail(&self, message: &str) -> ExitCode {
        if self.json {
            print_json(&json::error_without_place(message));
        } else {
            err(&format!("tay plan: {message}\n"));
        }
        ExitCode::from(ERRORS)
    }
}

fn print_json(value: &serde_json::Value) {
    let json = serde_json::to_string_pretty(value).expect("a JSON value always serializes");
    out(&format!("{json}\n"));
}
