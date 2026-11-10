//! Modules: the mechanisms units use. Each module reads its own settings and
//! describes the unit in a [`UnitShape`]; nothing outside a module's own file
//! and the list below knows which modules exist.

mod made;
mod running;
pub mod shape;
mod want;

use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Loc;
use crate::syntax::yaml::Node;

pub use shape::{Concern, GivenValue, Place, RestartDefault, Runs, UnitShape};
pub use want::{Form, Want};

/// One mechanism a unit can use.
pub trait Module: Sync {
    /// The name a unit's `uses` gives.
    fn name(&self) -> &'static str;

    /// The settings that map names to values, which a way's `with` joins
    /// with its unit's entry by entry rather than replacing.
    fn joined_by_entry(&self) -> &'static [&'static str] {
        &[]
    }

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
    &running::process::Process,
    &running::container::Container,
    &running::command::Command,
    &made::port::Port,
    &made::file::File,
    &made::secret::Secret,
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
    use crate::diagnostic::source::{Loc, Sources};
    use crate::syntax::yaml;

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
