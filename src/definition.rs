//! A stack's definitions as read: what the reader gives the planner.

use std::path::PathBuf;

use crate::expr::{Expr, Type};
use crate::module::{GivenValue, Module, UnitShape};
use crate::names::NameAt;
use crate::source::Loc;

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
    /// `.tay/choices.yaml`, when the environment has one.
    pub environment_choices: Option<Choices>,
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

#[derive(Debug)]
pub struct Unit {
    pub name: NameAt,
    pub doc: Option<String>,
    pub action: bool,
    /// One variant for a unit with one way to run (or a group), one per
    /// alternative for a unit with `run`. Empty when the unit's definition
    /// could not be read, so that other units may still name it.
    pub variants: Vec<Variant>,
}

impl Unit {
    pub fn has_alternatives(&self) -> bool {
        self.variants
            .first()
            .is_some_and(|variant| variant.alternative.is_some())
    }

    pub fn alternative(&self, name: &str) -> Option<usize> {
        self.variants.iter().position(|variant| {
            variant
                .alternative
                .as_ref()
                .is_some_and(|alt| alt.name == name)
        })
    }

    pub fn alternative_names(&self) -> Vec<&str> {
        self.variants
            .iter()
            .filter_map(|variant| variant.alternative.as_ref().map(|alt| alt.name.as_str()))
            .collect()
    }
}

/// One way a unit runs, with the unit's own needs, waits and values joined in.
#[derive(Debug)]
pub struct Variant {
    pub alternative: Option<NameAt>,
    /// `None` for a group.
    pub module: Option<ModuleUse>,
    pub needs: Vec<NameAt>,
    pub after: Vec<NameAt>,
    pub gives: Vec<Give>,
    pub outputs: Vec<NameAt>,
    pub ready: Option<Ready>,
    pub restart: Option<Restart>,
    pub watch: Vec<Expr>,
}

impl Variant {
    pub fn give(&self, name: &str) -> Option<&Give> {
        self.gives.iter().find(|give| give.name.name == name)
    }
}

pub struct ModuleUse {
    pub module: &'static dyn Module,
    /// Where `uses` names the module.
    pub uses: Loc,
    pub shape: UnitShape,
}

impl std::fmt::Debug for ModuleUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleUse")
            .field("module", &self.module.name())
            .field("uses", &self.uses)
            .field("shape", &self.shape)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct Give {
    pub name: NameAt,
    pub value: GiveValue,
}

#[derive(Clone, Debug)]
pub enum GiveValue {
    Value(Expr),
    Endpoint(Box<Endpoint>),
    /// A value the unit's module gives.
    Module(GivenValue),
}

/// A value written as a mapping with `port`: an address whose host part
/// depends on where the unit that uses it runs.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub loc: Loc,
    pub port: Expr,
    pub scheme: Option<Expr>,
    pub path: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct Ready {
    pub check: Check,
    pub within: Expr,
}

#[derive(Clone, Debug)]
pub enum Check {
    Http(Expr),
    Tcp(Expr),
    Command(Vec<Expr>),
    Log(Expr),
}

#[derive(Clone, Debug)]
pub struct Restart {
    pub on: Option<Expr>,
    pub attempts: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct SettingDecl {
    pub name: NameAt,
    pub default: Option<Expr>,
    /// `None` only when the default is a single reference and no `type` is
    /// written: the type is then that of what the default refers to.
    pub ty: Option<Type>,
    pub secret: bool,
}

#[derive(Debug)]
pub struct Profile {
    pub name: NameAt,
    /// Where `production: true` is written, if it is.
    pub production: Option<Loc>,
    pub choices: Choices,
}

/// Choices as one place writes them: a profile, the personal file, the
/// environment's file, or one `--set`.
#[derive(Clone, Debug, Default)]
pub struct Choices {
    pub up: Option<Vec<NameAt>>,
    pub up_add: Vec<NameAt>,
    pub up_remove: Vec<NameAt>,
    pub run_default: Option<NameAt>,
    /// A unit, and the alternative chosen for it.
    pub run_units: Vec<(NameAt, NameAt)>,
    pub settings: Vec<(NameAt, SettingValue)>,
}

#[derive(Clone, Debug)]
pub enum SettingValue {
    Written(Expr),
    /// The text of a `--set`, typed once the setting's type is known.
    Argument(Expr),
}

impl SettingValue {
    pub fn expr(&self) -> &Expr {
        match self {
            SettingValue::Written(expr) | SettingValue::Argument(expr) => expr,
        }
    }
}
