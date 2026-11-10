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
    pub ready_without_check: &'static str,
    pub restart_by_default: RestartDefault,
}

impl UnitShape {
    /// A unit's shape with only its kind and its defaults: what it uses,
    /// gives and makes is filled in by its module.
    pub fn new(
        kind: UnitKind,
        ready_without_check: &'static str,
        restart_by_default: RestartDefault,
    ) -> UnitShape {
        UnitShape {
            kind,
            slots: Vec::new(),
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
    /// `secret_env`: only values with a secret in them.
    SecretEnv,
    /// A `file` unit's content, which is written for its owner alone when
    /// it holds a secret and no `mode` is written.
    FileContent,
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
    /// Generated once and kept: known before start, and never shown when
    /// it is secret.
    Generated { secret: bool },
    /// Written out by the unit when it runs.
    Produced { secret: bool },
    /// A file the unit builds when it runs.
    Output,
}

/// Something a production profile refuses.
#[derive(Clone, Debug)]
pub enum Concern {
    BuildsImage(Loc),
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
