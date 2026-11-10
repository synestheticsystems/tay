//! What a module tells the planner about a unit, in terms that name no module.

use crate::diagnostic::source::Loc;
use crate::syntax::expr::Expr;
use crate::syntax::names::NameAt;

use crate::diagnostic::Diagnostics;
use crate::syntax::expr;
use crate::syntax::yaml::Node;

use super::want::{Form, Want};

/// A module's account of one unit, read from the unit's settings. The planner
/// works from this alone.
#[derive(Clone, Debug)]
pub struct UnitShape {
    pub kind: UnitKind,
    /// Every value the unit's settings use, with what each must be and where
    /// it goes.
    pub slots: Vec<Slot>,
    /// The values the module gives, used as `${unit.name}`.
    pub gives: Vec<Given>,
    /// The files the unit builds, used as `${unit.outputs.name}`.
    pub outputs: Vec<Given>,
    /// What in these settings a production profile refuses, beyond running on
    /// the host, which the planner sees in `kind`.
    pub production: Vec<Concern>,
    /// How the unit is known to be ready when it has no `ready` check.
    pub ready_without_check: &'static str,
    pub restart_by_default: RestartDefault,
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
    /// by the unit's name and from the host through `published` ports.
    OwnContainer {
        published: Vec<Published>,
    },
    /// Inside another unit's container.
    InsideUnit(NameAt),
    /// In a container made for one run, on the environment's network.
    OneOffContainer,
}

/// A container port that may be made reachable from the host. It is opened
/// only when something in the plan uses it from the host, or when `always`
/// says so; until then nothing uses its values.
#[derive(Clone, Debug)]
pub struct Published {
    pub container_port: i64,
    pub host_port: Slot,
    /// The host address it is opened on; `None` for 127.0.0.1.
    pub address: Option<Slot>,
    pub always: bool,
}

/// One value a unit's settings use.
#[derive(Clone, Debug)]
pub struct Slot {
    pub expr: Expr,
    pub want: Want,
    pub place: Place,
}

impl Slot {
    /// Reads a setting's value. Its written form is checked now; a reference
    /// in it is checked once it is resolved.
    pub fn read(node: &Node, want: Want, place: Place, d: &mut Diagnostics) -> Option<Slot> {
        let expr = expr::from_node(node, d)?;
        if let Some(problem) = Form::of_written(&expr).and_then(|form| want.problem(&form)) {
            d.error(node.loc, format!("this {problem}"));
            return None;
        }
        Some(Slot { expr, want, place })
    }
}

/// Where a value ends up, which decides whether a secret may go there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// `secret_env`: only secrets, only by reference.
    SecretEnv,
    /// A file only its owner can read.
    PrivateFile,
    /// A place other programs can read, named for messages.
    Readable(&'static str),
}

/// A value a module gives.
#[derive(Clone, Debug)]
pub struct Given {
    pub name: NameAt,
    pub value: GivenValue,
}

#[derive(Clone, Debug)]
pub enum GivenValue {
    /// Built from other values, all known before start unless one is produced.
    Expr(Expr),
    /// A host port this environment holds: a number, known before start,
    /// shown as `<port: unit>` until it is allocated.
    Port,
    /// A generated secret: known before start, never shown.
    Secret,
    /// Written out by the unit when it runs.
    Produced,
    /// A file the unit builds when it runs.
    Output,
}

/// Something a production profile refuses.
#[derive(Clone, Debug)]
pub enum Concern {
    BuildsImage(Loc),
    Prepares(Loc),
    /// A host path made visible inside a container.
    MountsHostPath(Expr),
}

/// What a unit restarts on when its `restart` does not say: failure for a
/// unit that keeps running, never for one that runs once or is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartDefault {
    OnFailure,
    Never,
}

impl RestartDefault {
    /// The word `restart.on` would be written with.
    pub fn word(self) -> &'static str {
        match self {
            RestartDefault::OnFailure => "failure",
            RestartDefault::Never => "never",
        }
    }
}
