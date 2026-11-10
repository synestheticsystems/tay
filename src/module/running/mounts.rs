//! Mounts: a host file or folder, or another unit's volume, made visible in
//! a container.

use crate::diagnostic::Diagnostics;
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::Node;

use crate::module::description::{Concern, VolumeMount};
use crate::module::settings::{Destination, Slot, maybe};
use crate::module::want::{PathUse, Want};

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
    Volume(VolumeMount),
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

impl Mount<Slot> {
    /// What a production profile refuses in this mount: a host path.
    pub fn concern(&self) -> Option<Concern> {
        match &self.source {
            Source::Host(from) => Some(Concern::MountsHostPath(from.expr.clone())),
            Source::Volume(_) => None,
        }
    }

    /// The other unit's volume this mount makes visible.
    pub fn volume(&self) -> Option<&VolumeMount> {
        match &self.source {
            Source::Volume(volume) => Some(volume),
            Source::Host(_) => None,
        }
    }
}

/// `mounts`: each a host file or folder made visible in a container,
/// `{ from, to, readonly }`, or a volume another unit owns,
/// `{ volume: "unit/name", to, readonly }`.
pub fn mounts(node: &Node, d: &mut Diagnostics) -> Vec<Mount<Slot>> {
    let Some(items) = fields::list(node, "`mounts`", d) else {
        return Vec::new();
    };
    let mut mounts = Vec::new();
    for item in items {
        let Some(mount) = Fields::of(item, "a mount", &["from", "volume", "to", "readonly"], d)
        else {
            continue;
        };
        let source = match (mount.entry("from"), mount.entry("volume")) {
            (Some((_, from)), None) => {
                Slot::read(from, Want::Path(PathUse::Mounted), MOUNT, d).map(Source::Host)
            }
            (None, Some((_, volume))) => volume_mount(volume, d).map(Source::Volume),
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
    mounts
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
