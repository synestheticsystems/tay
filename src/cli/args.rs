//! Reading a command's arguments: the options it takes, and the words that
//! are not options: the units it may name, or what else it takes.

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
    /// Whether it takes `--since` and `--follow`, as it reads a log.
    pub follows: bool,
    /// Whether it takes `--all` and `--yes`, as it removes what is kept.
    pub wipes: bool,
    /// How many words it takes that are not options.
    pub words: Words,
}

pub enum Words {
    None,
    /// At most one, the unit it names.
    AtMostOneUnit,
    Any,
}

/// A command's arguments as read.
#[derive(Default)]
pub struct Args {
    pub root: Option<PathBuf>,
    pub profile: Option<String>,
    pub sets: Vec<String>,
    pub json: bool,
    /// The words that are not options: the units a command names, or what
    /// else it takes, as the places `tay set` sets.
    pub words: Vec<String>,
    /// How far back to read a log, as `10m`.
    pub since: Option<String>,
    pub follow: bool,
    /// Everything the environment keeps, rather than what units keep.
    pub all: bool,
    /// What is kept is removed, rather than said.
    pub yes: bool,
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
                "--since" if self.follows => {
                    parsed.since = Some(text(value("--since")?, "--since")?);
                }
                "--follow" if self.follows && inline.is_none() => parsed.follow = true,
                "--all" if self.wipes && inline.is_none() => parsed.all = true,
                "--yes" if self.wipes && inline.is_none() => parsed.yes = true,
                word if !word.starts_with('-') && self.takes_another_word(&parsed) => {
                    parsed.words.push(word.to_string());
                }
                unit if !unit.starts_with('-') && matches!(self.words, Words::AtMostOneUnit) => {
                    return Err(format!(
                        "`{command}` names at most one unit; `{unit}` is a second"
                    ));
                }
                _ => return Err(format!("`{arg_text}` is not an option of `{command}`")),
            }
        }
        Ok(Some(parsed))
    }

    fn takes_another_word(&self, parsed: &Args) -> bool {
        match self.words {
            Words::None => false,
            Words::AtMostOneUnit => parsed.words.is_empty(),
            Words::Any => true,
        }
    }
}
