//! Modules: the mechanisms units use. Each module reads its own settings and
//! describes the unit in a [`UnitShape`]; nothing outside a module's own file
//! and the list below knows which modules exist.

mod command;
mod common;
mod container;
mod file;
mod port;
mod process;
mod secret;
pub mod shape;

use crate::diagnostic::Diagnostics;
use crate::source::Loc;
use crate::yaml::Node;

pub use shape::{
    Concern, Form, Given, GivenValue, Place, Published, RestartDefault, Runs, Slot, UnitShape, Want,
};

/// One mechanism a unit can use.
pub trait Module: Sync {
    /// The name a unit's `uses` gives.
    fn name(&self) -> &'static str;

    /// Reads the unit's `with` strictly against this module's settings and
    /// describes the unit. `None` when the settings have errors, which are
    /// reported through `cx`.
    fn check(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<UnitShape>;
}

/// What a module is told about the unit whose settings it reads.
pub struct Cx<'a> {
    pub unit: &'a str,
    /// Where the unit says `uses`, for errors about the settings as a whole.
    pub uses: Loc,
    pub d: &'a mut Diagnostics,
}

/// Every module that ships with tay.
static MODULES: [&dyn Module; 6] = [
    &process::Process,
    &container::Container,
    &command::Command,
    &port::Port,
    &file::File,
    &secret::Secret,
];

pub fn find(name: &str) -> Option<&'static dyn Module> {
    MODULES.iter().copied().find(|module| module.name() == name)
}

pub fn names() -> impl Iterator<Item = &'static str> {
    MODULES.iter().map(|module| module.name())
}

#[cfg(test)]
pub(crate) mod testing {
    use crate::diagnostic::Diagnostics;
    use crate::source::{Loc, Sources};
    use crate::yaml;

    use super::{Cx, UnitShape};

    /// Checks `with` (YAML text) for a unit using `module`, and gives the
    /// shape and each error as `line:column: message`.
    pub fn check(module: &str, with: &str) -> (Option<UnitShape>, Vec<String>) {
        let mut sources = Sources::default();
        let id = sources.add_argument("with");
        let mut d = Diagnostics::default();
        let node = yaml::parse(with, id, &mut d).expect("test YAML parses");
        let mut cx = Cx {
            unit: "the-unit",
            uses: Loc::new(id, 1, 1),
            d: &mut d,
        };
        let shape = super::find(module).unwrap().check(Some(&node), &mut cx);
        let errors = d
            .into_sorted()
            .iter()
            .map(|e| format!("{}:{}: {}", e.loc.line, e.loc.column, e.message))
            .collect();
        (shape, errors)
    }
}
