//! Reading a container unit's `with` into its settings and published ports.

use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Loc;
use crate::syntax::fields::{self, Fields};
use crate::syntax::names;
use crate::syntax::yaml::{Kind, Node, Scalar};

use super::{ContainerSettings, Image, Volume};
use crate::module::Cx;
use crate::module::description::{PublishEntry, VolumeMount};
use crate::module::running::command_line::{COMMAND_LINE, argv, words};
use crate::module::running::mounts::mounts;
use crate::module::running::variables::variables;
use crate::module::settings::{Destination, Slot};
use crate::module::want::{PathUse, Want};

const FIELDS: &[&str] = &[
    "image",
    "build",
    "command",
    "entrypoint",
    "env",
    "secret_env",
    "volumes",
    "mounts",
    "tmpfs",
    "publish",
    "aliases",
    "labels",
    "user",
    "workdir",
    "platform",
    "read_only",
    "security_opt",
    "pids_limit",
];

const SETTING: Destination = Destination::Readable("a container's settings");

/// What a container unit's `with` says.
pub struct Read {
    pub settings: ContainerSettings<Slot>,
    /// Each other unit's volume it mounts, where it is written.
    pub volume_mounts: Vec<VolumeMount>,
    pub publish: Vec<PublishEntry>,
    /// Where `build` is written, when the image is built.
    pub built_at: Option<Loc>,
}

pub fn read(with: Option<&Node>, cx: &mut Cx<'_>) -> Option<Read> {
    let Some(with) = with else {
        cx.d.error(
            cx.uses,
            "a `container` unit needs `with` and in it an `image` or a `build`",
        );
        return None;
    };
    let errors_before = cx.d.len();
    let d = &mut *cx.d;
    let settings = Fields::of(with, "the settings of a `container` unit", FIELDS, d)?;
    let publish = settings
        .get("publish")
        .map(|node| publish_entries(node, d))
        .unwrap_or_default();
    let (image, built_at) = match (settings.entry("image"), settings.entry("build")) {
        (Some((_, image)), None) => (
            Slot::read(image, Want::Text, Destination::Readable("an image name"), d)
                .map(Image::Named),
            None,
        ),
        (None, Some((key, build))) => (build_settings(build, d), Some(key.loc)),
        (Some(_), Some((key, _))) => {
            d.error(
                key.loc,
                "a `container` unit has exactly one of `image` and `build`; this one has both",
            );
            (None, None)
        }
        (None, None) => {
            d.error(
                settings.loc(),
                "a `container` unit needs exactly one of `image` and `build`",
            );
            (None, None)
        }
    };
    let command = settings
        .get("command")
        .and_then(|node| argv(node, "`command`", COMMAND_LINE, d));
    // An empty entrypoint clears the image's own.
    let entrypoint = settings
        .get("entrypoint")
        .and_then(|node| words(node, "`entrypoint`", COMMAND_LINE, d));
    let variables = variables(&settings, d);
    let volumes = settings
        .get("volumes")
        .map(|volumes| volume_settings(volumes, d))
        .unwrap_or_default();
    let (mounts, volume_mounts) = settings
        .get("mounts")
        .map(|node| mounts(node, d))
        .unwrap_or_default();
    let tmpfs = text_list(&settings, "tmpfs", d);
    let aliases = settings
        .get("aliases")
        .map(|aliases| {
            fields::name_list(aliases, "an alias", names::name_problem, d)
                .into_iter()
                .map(|alias| alias.name)
                .collect()
        })
        .unwrap_or_default();
    let labels = settings
        .get("labels")
        .map(|labels| {
            fields::keyed(labels, "a label", label_key_problem, d)
                .into_iter()
                .filter_map(|(key, label)| {
                    let label =
                        Slot::read(label, Want::Scalar, Destination::Readable("a label"), d)?;
                    Some((key.name, label))
                })
                .collect()
        })
        .unwrap_or_default();
    let [user, workdir, platform, read_only, pids_limit] = [
        ("user", Want::Scalar),
        ("workdir", Want::Text),
        ("platform", Want::Text),
        ("read_only", Want::Bool),
        ("pids_limit", Want::Number),
    ]
    .map(|(field, want)| {
        settings
            .get(field)
            .and_then(|node| Slot::read(node, want, SETTING, d))
    });
    let security_opt = text_list(&settings, "security_opt", d);
    if cx.d.len() != errors_before {
        return None;
    }
    let settings = ContainerSettings {
        image: image?,
        command,
        entrypoint,
        variables,
        volumes,
        mounts,
        tmpfs,
        aliases,
        labels,
        user,
        workdir,
        platform,
        read_only,
        security_opt,
        pids_limit,
    };
    Some(Read {
        settings,
        volume_mounts,
        publish,
        built_at,
    })
}

/// A setting that is a list of text, such as `tmpfs`.
fn text_list(settings: &Fields<'_>, field: &'static str, d: &mut Diagnostics) -> Vec<Slot> {
    let Some(items) = settings
        .get(field)
        .and_then(|node| fields::list(node, &format!("`{field}`"), d))
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| Slot::read(item, Want::Text, SETTING, d))
        .collect()
}

fn build_settings(node: &Node, d: &mut Diagnostics) -> Option<Image<Slot>> {
    const BUILD: Destination = Destination::Readable("an image build");
    let build = Fields::of(node, "`build`", &["context", "dockerfile", "args"], d)?;
    let context = build
        .require("context", d)
        .and_then(|n| Slot::read(n, Want::Path(PathUse::Named), BUILD, d));
    let dockerfile = build
        .get("dockerfile")
        .and_then(|n| Slot::read(n, Want::Text, BUILD, d));
    let args = build
        .get("args")
        .map(|args| {
            fields::keyed(args, "a build argument", names::variable_name_problem, d)
                .into_iter()
                .filter_map(|(name, arg)| {
                    let arg = Slot::read(
                        arg,
                        Want::Scalar,
                        Destination::Readable("an image build argument"),
                        d,
                    )?;
                    Some((name.name, arg))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Image::Built {
        context: context?,
        dockerfile,
        args,
    })
}

/// `volumes`: the named volumes the unit owns, each a path in the
/// container, or `{ to, owner }`.
fn volume_settings(node: &Node, d: &mut Diagnostics) -> Vec<Volume<Slot>> {
    const VOLUME: Destination = Destination::Readable("a volume");
    let mut volumes = Vec::new();
    for (name, volume) in fields::keyed(node, "a volume", names::name_problem, d) {
        if volume.as_map().is_none() {
            if let Some(to) = Slot::read(volume, Want::Text, VOLUME, d) {
                volumes.push(Volume {
                    name,
                    to,
                    owner: None,
                });
            }
            continue;
        }
        let Some(long) = Fields::of(volume, "a volume", &["to", "owner"], d) else {
            continue;
        };
        let to = long
            .require("to", d)
            .and_then(|n| Slot::read(n, Want::Text, VOLUME, d));
        let owner = match long.get("owner") {
            Some(n) => Slot::read(n, Want::Number, VOLUME, d).map(Some),
            None => Some(None),
        };
        if let (Some(to), Some(owner)) = (to, owner) {
            volumes.push(Volume { name, to, owner });
        }
    }
    volumes
}

/// `publish`: container ports, as keys, each made reachable at a host port
/// when it is opened. Its values are not among the container's settings: an
/// entry that is not opened uses nothing.
fn publish_entries(node: &Node, d: &mut Diagnostics) -> Vec<PublishEntry> {
    const PUBLISHED: Destination = Destination::Readable("a published port");
    // Nothing written publishes no port, as any mapping of names with
    // nothing written names nothing.
    if node.nothing_written() {
        return Vec::new();
    }
    let Some(map) = node.as_map() else {
        d.error(
            node.loc,
            format!(
                "`publish` must be a mapping from a container port to a host port; this is {}",
                node.describe()
            ),
        );
        return Vec::new();
    };
    let mut entries = Vec::new();
    for entry in map {
        let container_port = match entry.key.kind {
            Kind::Scalar(Scalar::Int(port)) if (1..=65535).contains(&port) => port,
            _ => {
                d.error(
                    entry.key.loc,
                    format!(
                        "a key of `publish` is a container port, a number from 1 to 65535; this is {}",
                        entry.key.describe()
                    ),
                );
                continue;
            }
        };
        let (host_port, address, always) = if entry.value.as_map().is_some() {
            let Some(long) = Fields::of(
                &entry.value,
                "a published port",
                &["port", "address", "always"],
                d,
            ) else {
                continue;
            };
            let address = long
                .get("address")
                .and_then(|n| Slot::read(n, Want::Text, PUBLISHED, d));
            let always = long
                .get("always")
                .and_then(|n| fields::boolean(n, "`always`", d))
                .unwrap_or(false);
            (long.require("port", d), address, always)
        } else {
            (Some(&entry.value), None, false)
        };
        let Some(host_port) = host_port.and_then(|n| Slot::read(n, Want::PortNumber, PUBLISHED, d))
        else {
            continue;
        };
        entries.push(PublishEntry {
            container_port,
            host_port,
            address,
            always,
        });
    }
    entries
}

fn label_key_problem(key: &str) -> Option<String> {
    if key.is_empty() || key.contains(char::is_whitespace) || key.contains("${") {
        return Some(format!(
            "`{key}` is not a label key: written out, with no spaces"
        ));
    }
    // What carries the labels tay sets decides what tay may touch, so a
    // definition does not set them.
    key.starts_with("tay.").then(|| {
        format!("`{key}` is not a label a definition sets: tay sets the labels that begin `tay.`")
    })
}
