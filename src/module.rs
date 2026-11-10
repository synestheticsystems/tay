//! Modules: the mechanisms units use. Each module reads a unit's `with` into
//! settings of its own type and describes the unit in a [`Description`];
//! nothing outside a module's own file and the list below knows which
//! modules exist.

mod description;
mod made;
mod operations;
mod running;
mod settings;
mod want;

use std::fmt;

use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Loc;
use crate::syntax::yaml::Node;

pub use description::{
    Concern, Description, ModuleValue, Output, PublishEntry, ReadyWithoutCheck, RestartOn, Runs,
    UnitKind, VolumeMount,
};
pub use operations::{Exists, Made, Operations, UnitPlace, Values};
pub use running::command_line::argv;
pub use running::container::{HOST_IN_CONTAINERS, host_listen};
pub use settings::{Destination, Settings, Slot};
pub use want::{Duration, Form, PathUse, Want};

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
    fn read(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<Description>;
}

impl fmt::Debug for dyn Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
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
    use crate::syntax::yaml::{self, Node};

    use super::{Cx, Description};

    /// Reads `with` (YAML text) with `read`, for a unit called `the-unit`,
    /// and gives what it read and each error as `line:column: message`.
    pub fn read<T>(with: &str, read: impl FnOnce(&Node, &mut Cx<'_>) -> T) -> (T, Vec<String>) {
        let mut sources = Sources::default();
        let id = sources.add_argument("with");
        let mut d = Diagnostics::default();
        let node = yaml::parse(with, id, &mut d).expect("test YAML parses");
        let mut cx = Cx {
            unit: "the-unit",
            uses: Loc::new(id, 1, 1),
            d: &mut d,
        };
        let read = read(&node, &mut cx);
        let errors = d
            .into_sorted()
            .iter()
            .map(|e| format!("{}:{}: {}", e.loc.line, e.loc.column, e.message))
            .collect();
        (read, errors)
    }

    /// Reads `with` for a unit using `module`, and gives its description and
    /// each error.
    pub fn check(module: &str, with: &str) -> (Option<Description>, Vec<String>) {
        read(with, |node, cx| {
            super::find(module).unwrap().read(Some(node), cx)
        })
    }
}
