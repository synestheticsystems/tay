//! What a module tells the planner about a unit, in terms that name no module.

use crate::diagnostic::source::Loc;
use crate::syntax::expr::Expr;
use crate::syntax::names::NameAt;

use super::settings::{Slot, WrittenSettings};

/// A module's description of one unit, read from the unit's `with`: its own
/// settings, and what the planner needs to know of the unit, which it works
/// from alone.
#[derive(Debug)]
pub struct Description {
    pub kind: UnitKind,
    /// The module's own settings, every value still to be resolved.
    pub settings: Box<dyn WrittenSettings>,
    /// The ports a unit in a container of its own publishes to the host.
    pub publish: Vec<PublishEntry>,
    /// The values the module gives, used as `${unit.name}`. Reading the unit
    /// moves them in among the values the unit gives.
    pub gives: Vec<ModuleGive>,
    /// The files the unit builds, used as `${unit.outputs.name}`.
    pub outputs: Vec<Output>,
    /// The named volumes the unit owns.
    pub volumes: Vec<NameAt>,
    /// The volumes of other units it mounts.
    pub volume_mounts: Vec<VolumeMount>,
    /// What the unit makes that no other unit may make too.
    pub claims: Vec<Claim>,
    /// What in these settings a production profile refuses, beyond running on
    /// the host, which the planner sees in `kind`.
    pub production: Vec<Concern>,
    /// How the unit is known to be ready when it has no `ready` check.
    pub ready_without_check: ReadyWithoutCheck,
    /// What the unit restarts on when its `restart` does not say.
    pub restart_by_default: RestartOn,
}

impl Description {
    /// A unit's description with only its kind, its settings and its
    /// defaults: what it gives and makes is filled in by its module.
    pub fn new(
        kind: UnitKind,
        settings: Box<dyn WrittenSettings>,
        ready_without_check: ReadyWithoutCheck,
        restart_by_default: RestartOn,
    ) -> Description {
        Description {
            kind,
            settings,
            publish: Vec::new(),
            gives: Vec::new(),
            outputs: Vec::new(),
            volumes: Vec::new(),
            volume_mounts: Vec::new(),
            claims: Vec::new(),
            production: Vec::new(),
            ready_without_check,
            restart_by_default,
        }
    }
}

/// Another unit's volume, mounted: `{ volume: "unit/name" }`.
#[derive(Clone, Debug)]
pub struct VolumeMount {
    pub unit: NameAt,
    pub volume: NameAt,
}

/// Something in the environment a unit makes, described for messages, such
/// as a file at its place in the state folder. Two units may not make the
/// same one.
#[derive(Clone, Debug)]
pub struct Claim {
    pub what: String,
    pub loc: Loc,
}

/// The module's answers to the two questions the planner asks of every
/// unit: is it something that runs or something that is made, and where
/// does it run.
#[derive(Clone, Debug)]
pub enum UnitKind {
    /// It runs, where `Runs` says. Using an address it gives orders nothing.
    Runs(Runs),
    /// It is made and then simply exists: a port held, a file written, a
    /// secret generated. It runs nowhere, and using any of its values is a
    /// need for it.
    Made,
}

/// Where a unit that runs runs. It decides which view of an address the
/// unit's own settings receive, and how others reach an address the unit
/// gives.
#[derive(Clone, Debug)]
pub enum Runs {
    OnHost,
    /// In a container of its own on the environment's network, reached there
    /// by the unit's name and from the host through the ports it publishes.
    OwnContainer,
    /// Inside another unit's container.
    InsideUnit(NameAt),
    /// In a container made for one run, on the environment's network.
    OneOffContainer,
}

/// One entry of a container's `publish`: a container port that may be made
/// reachable from the host. It is opened only when something in the plan
/// uses it from the host, or when `always` says so; until then nothing uses
/// its values.
#[derive(Clone, Debug)]
pub struct PublishEntry {
    pub container_port: i64,
    pub host_port: Slot,
    /// The host address it is opened on; `None` for 127.0.0.1.
    pub address: Option<Slot>,
    pub always: bool,
}

/// A published port the plan opens, with where on the host it is opened:
/// what a unit in a container of its own is reached by from the host. Only
/// such a unit has any.
#[derive(Clone, Debug)]
pub struct OpenedPort<V> {
    pub container_port: i64,
    pub address: V,
    pub host_port: V,
}

/// A file a unit builds, and its path on the host.
#[derive(Clone, Debug)]
pub struct Output {
    pub name: NameAt,
    pub path: Expr,
}

/// A value a module gives.
#[derive(Clone, Debug)]
pub struct ModuleGive {
    pub name: NameAt,
    pub value: ModuleValue,
}

#[derive(Clone, Debug)]
pub enum ModuleValue {
    /// Built from other values, all known before start unless one is produced.
    Expr(Expr),
    /// A host port this environment holds: a number, known before start,
    /// shown as `<port: unit>` until it is allocated.
    Port,
    /// Generated once and kept: known before start, and never shown when
    /// it is secret.
    Generated { secret: bool },
    /// Written out by the unit when it runs.
    Produced { secret: bool },
}

/// Something a production profile refuses.
#[derive(Clone, Debug)]
pub enum Concern {
    BuildsImage(Loc),
    /// A host path made visible inside a container.
    MountsHostPath(Expr),
}

/// How a unit with no `ready` is known to be ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadyWithoutCheck {
    /// It has started.
    Started,
    /// Its image's own health check passes, or it has started if the image
    /// has none.
    Healthy,
    /// It has exited with 0.
    Exited,
    /// Its port is held.
    Held,
    /// It has been generated.
    Generated,
    /// It has been written.
    Written,
}

impl ReadyWithoutCheck {
    /// How a person reads it.
    pub fn words(self) -> &'static str {
        match self {
            ReadyWithoutCheck::Started => "when it has started",
            ReadyWithoutCheck::Healthy => {
                "when its image's own health check passes, or when it has started if the image has none"
            }
            ReadyWithoutCheck::Exited => "when it exits with 0",
            ReadyWithoutCheck::Held => "when it is held",
            ReadyWithoutCheck::Generated => "when it has been generated",
            ReadyWithoutCheck::Written => "when it has been written",
        }
    }
}

/// When a unit that runs is started again: `restart.on`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartOn {
    Failure = 0,
    Always = 1,
    Never = 2,
}

impl RestartOn {
    const ALL: [RestartOn; 3] = [RestartOn::Failure, RestartOn::Always, RestartOn::Never];

    /// The words `restart.on` is written with, each at the index of its
    /// `RestartOn` in `ALL`.
    pub const WORDS: &[&str] = &["failure", "always", "never"];

    pub fn word(self) -> &'static str {
        Self::WORDS[self as usize]
    }

    pub fn of_word(word: &str) -> Option<RestartOn> {
        Self::ALL.into_iter().find(|on| on.word() == word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_restart_is_read_from_the_word_it_is_written_with() {
        for on in RestartOn::ALL {
            assert_eq!(RestartOn::of_word(on.word()), Some(on));
        }
        assert_eq!(RestartOn::of_word("sometimes"), None);
    }
}
