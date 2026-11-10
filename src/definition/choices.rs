//! Choices as the places that make them write them, and the profiles that
//! hold some of them.

use crate::diagnostic::source::Loc;
use crate::syntax::names::NameAt;

use super::SettingValue;

#[derive(Debug)]
pub struct Profile {
    pub name: NameAt,
    /// Where `production: true` is written, if it is.
    pub production: Option<Loc>,
    /// Settings a choice must give a value that is not empty.
    pub require: Vec<NameAt>,
    pub choices: Choices,
}

/// Choices as one place writes them: `tay.yaml`, a profile, the personal
/// file, the environment's file, or one `--set`.
#[derive(Clone, Debug, Default)]
pub struct Choices {
    pub up: Option<Vec<NameAt>>,
    pub up_add: Vec<NameAt>,
    pub up_remove: Vec<NameAt>,
    /// Way names in the order they are tried.
    pub run_default: Option<Vec<NameAt>>,
    /// A unit, and the way chosen for it.
    pub run_units: Vec<(NameAt, NameAt)>,
    pub settings: Vec<(NameAt, SettingValue)>,
    /// The environment's label, in place of the one worked out from the
    /// folder's name.
    pub label: Option<String>,
}
