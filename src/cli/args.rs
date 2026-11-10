//! Reading a command's arguments: the options it takes, and the units it
//! may name.

use std::ffi::OsString;
use std::path::PathBuf;

/// What one command takes.
pub struct Takes {
    /// The command, as `tay plan` writes it in messages.
    pub command: &'static str,
    /// Whether it takes `--profile` and `--set`, and so plans as `tay plan`
    /// does.
    pub choices: bool,
    /// Whether it takes `--json`.
    pub json: bool,
    /// How many units it may name.
    pub units: UnitsNamed,
}

pub enum UnitsNamed {
    None,
    AtMostOne,
    Any,
}

/// A command's arguments as read.
#[derive(Default)]
pub struct Args {
    pub root: Option<PathBuf>,
    pub profile: Option<String>,
    pub sets: Vec<String>,
    pub json: bool,
    pub units: Vec<String>,
}

impl Takes {
    /// Reads the arguments after the command's name; `None` when help is
    /// asked for.
    pub fn read(&self, args: impl Iterator<Item = OsString>) -> Result<Option<Args>, String> {
        let command = self.command;
        let mut parsed = Args::default();
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
                "--json" if self.json && inline.is_none() => parsed.json = true,
                "--root" => parsed.root = Some(PathBuf::from(value("--root")?)),
                "--profile" if self.choices => {
                    parsed.profile = Some(text(value("--profile")?, "--profile")?);
                }
                "--set" if self.choices => parsed.sets.push(text(value("--set")?, "--set")?),
                unit if !unit.starts_with('-') && self.takes_another_unit(&parsed) => {
                    parsed.units.push(unit.to_string());
                }
                unit if !unit.starts_with('-') && matches!(self.units, UnitsNamed::AtMostOne) => {
                    return Err(format!(
                        "`{command}` names at most one unit; `{unit}` is a second"
                    ));
                }
                _ => return Err(format!("`{arg_text}` is not an option of `{command}`")),
            }
        }
        Ok(Some(parsed))
    }

    fn takes_another_unit(&self, parsed: &Args) -> bool {
        match self.units {
            UnitsNamed::None => false,
            UnitsNamed::AtMostOne => parsed.units.is_empty(),
            UnitsNamed::Any => true,
        }
    }
}
