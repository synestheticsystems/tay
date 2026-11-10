//! Mounts: a host file or folder, or another unit's volume, made visible in
//! a container. Where a mount is written is told to the planner as it is
//! read; the settings hold only what is mounted, so that a mount can be
//! kept and had again as it was filled.

use serde::{Deserialize, Serialize};

use crate::diagnostic::Diagnostics;
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::Node;

use crate::module::description::{Concern, VolumeMount};
use crate::module::operations::{self, WrongType};
use crate::module::settings::{Destination, Slot, maybe};
use crate::module::want::{PathUse, Want};
use crate::value::{Exact, Filled};

const MOUNT: Destination = Destination::Readable("a mount");

/// A host file or folder, or another unit's volume, made visible in a
/// container.
#[derive(Debug)]
pub struct Mount<V> {
    pub source: Source<V>,
    pub to: V,
    /// `None`: read-only, the default.
    pub readonly: Option<V>,
}

/// What a mount makes visible.
#[derive(Debug)]
pub enum Source<V> {
    /// A file or folder on the host.
    Host(V),
    /// A volume another unit owns.
    Volume(UnitVolume),
}

/// A volume another unit owns: that unit, and the volume's name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitVolume {
    pub unit: String,
    pub volume: String,
}

impl<V> Mount<V> {
    pub fn map<'a, W>(&'a self, f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<Mount<W>> {
        let source = match &self.source {
            Source::Host(from) => f(from).map(Source::Host),
            Source::Volume(volume) => Some(Source::Volume(volume.clone())),
        };
        let to = f(&self.to);
        let readonly = maybe(&self.readonly, f);
        Some(Mount {
            source: source?,
            to: to?,
            readonly: readonly?,
        })
    }
}

impl Mount<Filled> {
    /// The mount with every value in it, as a fingerprint takes it.
    pub fn revealed(&self) -> serde_json::Value {
        let source = match &self.source {
            Source::Host(from) => serde_json::json!({ "from": from.reveal() }),
            Source::Volume(volume) => {
                serde_json::json!({ "volume": [&volume.unit, &volume.volume] })
            }
        };
        serde_json::json!({
            "source": source,
            "to": self.to.reveal(),
            "readonly": self.readonly.as_ref().map(Filled::reveal),
        })
    }
}

/// A mount as it was filled, kept so that it is had again as it was.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptMount {
    #[serde(flatten)]
    source: KeptSource,
    to: String,
    /// `None`: read-only, the default.
    readonly: Option<bool>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum KeptSource {
    From(String),
    Volume(UnitVolume),
}

impl Mount<Filled> {
    /// The mount as it is kept.
    pub fn kept(&self) -> Result<KeptMount, WrongType> {
        let source = match &self.source {
            Source::Host(from) => KeptSource::From(operations::text(from, "mounts")?.to_string()),
            Source::Volume(volume) => KeptSource::Volume(volume.clone()),
        };
        let readonly = (self.readonly.as_ref())
            .map(|readonly| operations::boolean(readonly, "mounts"))
            .transpose()?;
        Ok(KeptMount {
            source,
            to: operations::text(&self.to, "mounts")?.to_string(),
            readonly,
        })
    }
}

impl KeptMount {
    /// The mount as it was filled.
    pub fn filled(&self) -> Mount<Filled> {
        Mount {
            source: match &self.source {
                KeptSource::From(from) => Source::Host(Filled::text(from)),
                KeptSource::Volume(volume) => Source::Volume(volume.clone()),
            },
            to: Filled::text(&self.to),
            readonly: self
                .readonly
                .map(|readonly| Filled::of(Exact::Bool(readonly), false)),
        }
    }
}

/// What mounting host paths tells the planner: each path, which a
/// production profile judges.
pub fn concerns(mounts: &[Mount<Slot>]) -> Vec<Concern> {
    mounts
        .iter()
        .filter_map(|mount| match &mount.source {
            Source::Host(from) => Some(Concern::MountsHostPath(from.expr.clone())),
            Source::Volume(_) => None,
        })
        .collect()
}

/// `mounts`: each a host file or folder made visible in a container,
/// `{ from, to, readonly }`, or a volume another unit owns,
/// `{ volume: "unit/name", to, readonly }`. Gives the mounts, and each
/// other unit's volume they mount where it is written, which the planner
/// checks.
pub fn mounts(node: &Node, d: &mut Diagnostics) -> (Vec<Mount<Slot>>, Vec<VolumeMount>) {
    let mut mounts = Vec::new();
    let mut volumes = Vec::new();
    let Some(items) = fields::list(node, "`mounts`", d) else {
        return (mounts, volumes);
    };
    for item in items {
        let Some(mount) = Fields::of(item, "a mount", &["from", "volume", "to", "readonly"], d)
        else {
            continue;
        };
        let source = match (mount.entry("from"), mount.entry("volume")) {
            (Some((_, from)), None) => {
                Slot::read(from, Want::Path(PathUse::Mounted), MOUNT, d).map(Source::Host)
            }
            (None, Some((_, volume))) => volume_mount(volume, d).map(|written| {
                let volume = UnitVolume {
                    unit: written.unit.name.clone(),
                    volume: written.volume.name.clone(),
                };
                volumes.push(written);
                Source::Volume(volume)
            }),
            (Some(_), Some((key, _))) => {
                d.error(
                    key.loc,
                    "a mount has one of `from` and `volume`; this one has both",
                );
                None
            }
            (None, None) => {
                d.error(
                    mount.loc(),
                    "a mount needs `from`, a host file or folder, or `volume`, another unit's volume",
                );
                None
            }
        };
        let to = mount
            .require("to", d)
            .and_then(|n| Slot::read(n, Want::Text, MOUNT, d));
        let readonly = match mount.get("readonly") {
            Some(n) => Slot::read(n, Want::Bool, MOUNT, d).map(Some),
            None => Some(None),
        };
        if let (Some(source), Some(to), Some(readonly)) = (source, to, readonly) {
            mounts.push(Mount {
                source,
                to,
                readonly,
            });
        }
    }
    (mounts, volumes)
}

/// `"unit/name"`: the unit that owns the volume, and the volume's name.
fn volume_mount(node: &Node, d: &mut Diagnostics) -> Option<VolumeMount> {
    let text = fields::text(node, "`volume`", d)?;
    let Some((unit, volume)) = text.split_once('/') else {
        d.error(
            node.loc,
            format!("`volume` is written `unit/name`, the unit that owns it and its name; this is `{text}`"),
        );
        return None;
    };
    let name = |name: &str, d: &mut Diagnostics| match names::name_problem(name) {
        None => Some(NameAt {
            name: name.to_string(),
            loc: node.loc,
        }),
        Some(problem) => {
            d.error(node.loc, format!("`volume`: {problem}"));
            None
        }
    };
    let (unit, volume) = (name(unit, d), name(volume, d));
    Some(VolumeMount {
        unit: unit?,
        volume: volume?,
    })
}
