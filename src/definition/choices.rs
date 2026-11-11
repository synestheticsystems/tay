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

/// This environment's own file, `.tay/choices.yaml`: its choices, and the
/// profile it names, which no other file may.
#[derive(Debug)]
pub struct EnvironmentFile {
    pub profile: Option<NameAt>,
    pub choices: Choices,
}

/// Every place choices are written, in the order they are layered, later
/// ones winning.
#[derive(Debug)]
pub struct Places<'s> {
    /// `up` and `run` as `tay.yaml` writes them.
    pub defaults: &'s Choices,
    pub profile: Option<&'s Profile>,
    /// This person's own file, read only when the profile is not a
    /// production one.
    pub personal: Option<Choices>,
    /// This environment's `.tay/choices.yaml`.
    pub environment: Option<Choices>,
    /// Each `--set`, in the order written.
    pub sets: Vec<Choices>,
    /// The units `tay up` names, which replace `up`.
    pub named: Option<Choices>,
}

impl Places<'_> {
    pub fn in_order(&self) -> Vec<&Choices> {
        [self.defaults].into_iter().chain(self.made()).collect()
    }

    /// Every place but the stack's own defaults: the ones whose values are
    /// choices made for this environment, which is what a profile's
    /// `require` counts.
    pub fn made(&self) -> Vec<&Choices> {
        self.profile
            .map(|profile| &profile.choices)
            .into_iter()
            .chain(self.personal.as_ref())
            .chain(self.environment.as_ref())
            .chain(&self.sets)
            .chain(self.named.as_ref())
            .collect()
    }
}
