//! A stack's definitions as read: what the reader gives the planner.

mod choices;
pub mod read;
pub mod reference_errors;
mod setting;
mod unit;

use std::path::PathBuf;

use crate::diagnostic::source::Loc;
use crate::syntax::names::NameAt;

pub use choices::{Choices, Places, Profile};
pub use setting::{SettingDecl, SettingValue, Written};
pub use unit::{
    Action, Arg, Body, Check, Endpoint, Give, GiveValue, Mechanism, Ready, Restart, Unit, Way,
};

#[derive(Debug)]
pub struct Stack {
    pub root: PathBuf,
    /// `None` when `stack` is missing or not a name, which is reported.
    pub name: Option<NameAt>,
    /// Where `tay.yaml` is, for errors about the stack as a whole.
    pub loc: Loc,
    pub default_profile: Option<NameAt>,
    pub settings: Vec<SettingDecl>,
    pub units: Vec<Unit>,
    pub profiles: Vec<Profile>,
    /// `up` and `run` as `tay.yaml` writes them: the first place choices
    /// are layered from.
    pub defaults: Choices,
}

impl Stack {
    pub fn unit(&self, name: &str) -> Option<&Unit> {
        self.units.iter().find(|unit| unit.name.name == name)
    }

    pub fn setting(&self, name: &str) -> Option<&SettingDecl> {
        self.settings
            .iter()
            .find(|setting| setting.name.name == name)
    }
}
