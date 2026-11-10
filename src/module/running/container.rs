//! `container`: an image run through Docker, on the environment's network.

use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Loc;
use crate::syntax::fields::{self, Fields};
use crate::syntax::names::{self, NameAt};
use crate::syntax::yaml::{Kind, Node, Scalar};
use crate::value::Value;

use super::common::{Mount, Variables, argv, mounts, variables};
use crate::module::description::{
    Concern, Description, Published, ReadyWithoutCheck, RestartOn, Runs, UnitKind,
};
use crate::module::settings::{
    Destination, Settings, Slot, WrittenSettings, each, each_named, maybe,
};
use crate::module::want::{PathUse, Want};
use crate::module::{Cx, Module};

pub struct Container;

/// The name the host has inside the environment's containers, which Docker
/// gives it.
pub const HOST_IN_CONTAINERS: &str = "host.docker.internal";

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

/// A container's settings, each value a [`Slot`] as written or a [`Value`]
/// once resolved. Its published ports are in the unit's description, where
/// the planner decides which are opened.
#[derive(Debug)]
struct ContainerSettings<V> {
    image: Image<V>,
    /// `None`: the image's own.
    command: Option<Vec<V>>,
    /// `None`: the image's own.
    entrypoint: Option<Vec<V>>,
    variables: Variables<V>,
    volumes: Vec<Volume<V>>,
    mounts: Vec<Mount<V>>,
    tmpfs: Vec<V>,
    /// Further names on the environment's network, besides the unit's own.
    aliases: Vec<String>,
    labels: Vec<(String, V)>,
    user: Option<V>,
    workdir: Option<V>,
    platform: Option<V>,
    read_only: Option<V>,
    security_opt: Vec<V>,
    pids_limit: Option<V>,
}

/// The image a container runs.
#[derive(Debug)]
enum Image<V> {
    /// An image by name.
    Named(V),
    /// An image built from a folder on the host.
    Built {
        context: V,
        dockerfile: Option<V>,
        args: Vec<(String, V)>,
    },
}

/// A named volume the unit owns, and where it is in the container.
#[derive(Debug)]
struct Volume<V> {
    name: NameAt,
    to: V,
    /// The numeric user the volume is given to.
    owner: Option<V>,
}

impl<V> Image<V> {
    fn map<'a, W>(&'a self, f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<Image<W>> {
        match self {
            Image::Named(name) => f(name).map(Image::Named),
            Image::Built {
                context,
                dockerfile,
                args,
            } => {
                let context = f(context);
                let dockerfile = maybe(dockerfile, f);
                let args = each_named(args, f);
                Some(Image::Built {
                    context: context?,
                    dockerfile: dockerfile?,
                    args: args?,
                })
            }
        }
    }
}

impl<V> Volume<V> {
    fn map<'a, W>(&'a self, f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<Volume<W>> {
        let to = f(&self.to);
        let owner = maybe(&self.owner, f);
        Some(Volume {
            name: self.name.clone(),
            to: to?,
            owner: owner?,
        })
    }
}

impl<V> ContainerSettings<V> {
    fn map<'a, W>(&'a self, f: &mut dyn FnMut(&'a V) -> Option<W>) -> Option<ContainerSettings<W>> {
        let image = self.image.map(f);
        let command = maybe(&self.command, &mut |words| each(words, f));
        let entrypoint = maybe(&self.entrypoint, &mut |words| each(words, f));
        let variables = self.variables.map(f);
        let volumes: Vec<Option<Volume<W>>> = self.volumes.iter().map(|v| v.map(f)).collect();
        let mounts: Vec<Option<Mount<W>>> = self.mounts.iter().map(|m| m.map(f)).collect();
        let tmpfs = each(&self.tmpfs, f);
        let labels = each_named(&self.labels, f);
        let user = maybe(&self.user, f);
        let workdir = maybe(&self.workdir, f);
        let platform = maybe(&self.platform, f);
        let read_only = maybe(&self.read_only, f);
        let security_opt = each(&self.security_opt, f);
        let pids_limit = maybe(&self.pids_limit, f);
        Some(ContainerSettings {
            image: image?,
            command: command?,
            entrypoint: entrypoint?,
            variables: variables?,
            volumes: volumes.into_iter().collect::<Option<_>>()?,
            mounts: mounts.into_iter().collect::<Option<_>>()?,
            tmpfs: tmpfs?,
            aliases: self.aliases.clone(),
            labels: labels?,
            user: user?,
            workdir: workdir?,
            platform: platform?,
            read_only: read_only?,
            security_opt: security_opt?,
            pids_limit: pids_limit?,
        })
    }
}

impl WrittenSettings for ContainerSettings<Slot> {
    fn slots(&self) -> Vec<&Slot> {
        let mut slots = Vec::new();
        self.map(&mut |slot| {
            slots.push(slot);
            Some(())
        });
        slots
    }

    fn resolve(
        &self,
        resolve: &mut dyn FnMut(&Slot) -> Option<Value>,
    ) -> Option<Box<dyn Settings>> {
        Some(Box::new(self.map(resolve)?))
    }
}

impl Settings for ContainerSettings<Value> {}

impl Module for Container {
    fn name(&self) -> &'static str {
        "container"
    }

    fn joined_by_entry(&self) -> &'static [&'static str] {
        &["env", "secret_env", "labels", "volumes", "publish"]
    }

    fn read(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<Description> {
        let Read {
            settings,
            published,
            built_at,
        } = read(with, cx)?;
        let volumes = settings
            .volumes
            .iter()
            .map(|volume| volume.name.clone())
            .collect();
        let volume_mounts = settings
            .mounts
            .iter()
            .filter_map(|mount| mount.volume().cloned())
            .collect();
        let mut production: Vec<Concern> = built_at.map(Concern::BuildsImage).into_iter().collect();
        production.extend(settings.mounts.iter().filter_map(Mount::concern));
        Some(Description {
            published,
            volumes,
            volume_mounts,
            production,
            ..Description::new(
                UnitKind::Runs(Runs::OwnContainer),
                Box::new(settings),
                ReadyWithoutCheck::Healthy,
                RestartOn::Failure,
            )
        })
    }
}

/// What a container unit's `with` says.
struct Read {
    settings: ContainerSettings<Slot>,
    published: Vec<Published>,
    /// Where `build` is written, when the image is built.
    built_at: Option<Loc>,
}

fn read(with: Option<&Node>, cx: &mut Cx<'_>) -> Option<Read> {
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
    let published = settings
        .get("publish")
        .map(|node| publish_settings(node, d))
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
    let [command, entrypoint] = ["command", "entrypoint"].map(|list| {
        settings
            .get(list)
            .and_then(|node| argv(node, &format!("`{list}`"), d))
    });
    let variables = variables(&settings, d);
    let volumes = settings
        .get("volumes")
        .map(|volumes| volume_settings(volumes, d))
        .unwrap_or_default();
    let mounts = settings
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
        published,
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
fn publish_settings(node: &Node, d: &mut Diagnostics) -> Vec<Published> {
    const PUBLISHED: Destination = Destination::Readable("a published port");
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
    let mut published = Vec::new();
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
        published.push(Published {
            container_port,
            host_port,
            address,
            always,
        });
    }
    published
}

fn label_key_problem(key: &str) -> Option<String> {
    (key.is_empty() || key.contains(char::is_whitespace) || key.contains("${"))
        .then(|| format!("`{key}` is not a label key: written out, with no spaces"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::testing::{self, check};

    fn settings(with: &str) -> ContainerSettings<Slot> {
        let (read, errors) = testing::read(with, |node, cx| read(Some(node), cx));
        assert!(errors.is_empty(), "{errors:?}");
        read.unwrap().settings
    }

    #[test]
    fn exactly_one_of_image_and_build() {
        let (_, errors) = check("container", "command: [x]\n");
        assert_eq!(
            errors,
            ["1:1: a `container` unit needs exactly one of `image` and `build`"]
        );
        let (_, errors) = check("container", "image: a\nbuild: { context: . }\n");
        assert_eq!(
            errors,
            ["2:1: a `container` unit has exactly one of `image` and `build`; this one has both"]
        );
    }

    #[test]
    fn publish_keys_are_container_ports_and_values_host_ports() {
        let (description, errors) = check(
            "container",
            "image: db\npublish:\n  5432: \"${db-port.number}\"\n  8080: { port: 18080, address: 0.0.0.0, always: true }\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
        let description = description.unwrap();
        // A published port's values are used only once it is opened.
        assert!(
            description
                .settings
                .slots()
                .iter()
                .all(|slot| slot.want != Want::PortNumber)
        );
        assert!(matches!(
            description.kind,
            UnitKind::Runs(Runs::OwnContainer)
        ));
        assert_eq!(
            description
                .published
                .iter()
                .map(|p| (p.container_port, p.address.is_some(), p.always))
                .collect::<Vec<_>>(),
            [(5432, false, false), (8080, true, true)]
        );
        let (_, errors) = check("container", "image: db\npublish: { web: 80, 99999: 1 }\n");
        assert_eq!(errors.len(), 2);
        assert!(
            errors[0].starts_with("2:12: a key of `publish` is a container port"),
            "{errors:?}"
        );
        assert!(
            errors[1].starts_with("2:21: a key of `publish`"),
            "{errors:?}"
        );
    }

    #[test]
    fn a_mount_is_a_host_path_or_another_units_volume_and_volumes_are_owned() {
        let (description, errors) = check(
            "container",
            "image: app\nvolumes: { cache: /cache }\nmounts:\n  - { from: config, to: /config }\n  - { volume: database/data, to: /data }\ntmpfs: [/tmp]\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
        let description = description.unwrap();
        assert_eq!(description.volumes[0].name, "cache");
        let mounted = &description.volume_mounts[0];
        assert_eq!(
            (mounted.unit.name.as_str(), mounted.volume.name.as_str()),
            ("database", "data")
        );
        // Only the host path is something a production profile looks at.
        assert_eq!(description.production.len(), 1);
        let (_, errors) = check(
            "container",
            "image: app\nmounts:\n  - { volume: data, to: /data }\n  - { from: a, volume: b/c, to: /x }\n",
        );
        assert_eq!(
            errors,
            [
                "3:15: `volume` is written `unit/name`, the unit that owns it and its name; this is `data`",
                "4:16: a mount has one of `from` and `volume`; this one has both",
            ]
        );
    }

    #[test]
    fn a_build_is_a_production_concern_and_volumes_take_two_forms() {
        let (description, errors) = check(
            "container",
            "build: { context: ., args: { V: 1 } }\nvolumes: { data: /data, logs: { to: /logs, owner: 999 } }\nread_only: yes\n",
        );
        assert_eq!(errors, ["3:12: this must be true or false; this is text"]);
        assert!(description.is_none());
        let (description, errors) = check("container", "build: { context: . }\n");
        assert!(errors.is_empty(), "{errors:?}");
        let description = description.unwrap();
        assert_eq!(description.production.len(), 1);
        // The build's context is a host path.
        assert_eq!(
            description.settings.slots()[0].want,
            Want::Path(PathUse::Named)
        );
    }

    #[test]
    fn each_setting_is_read_into_its_place_and_every_value_is_resolved() {
        let read = settings(
            "build: { context: ., dockerfile: Dockerfile.dev, args: { V: 1 } }\ncommand: [serve]\nenv: { A: a }\nvolumes: { data: /data, logs: { to: /logs, owner: 999 } }\nmounts: [{ from: ./conf, to: /conf, readonly: false }]\ntmpfs: [/tmp]\naliases: [db]\nlabels: { team: core }\nuser: \"999\"\nworkdir: /app\nplatform: linux/amd64\nread_only: true\nsecurity_opt: [no-new-privileges]\npids_limit: 100\n",
        );
        let Image::Built {
            dockerfile, args, ..
        } = &read.image
        else {
            panic!("the image is built");
        };
        assert!(dockerfile.is_some());
        assert_eq!(args[0].0, "V");
        assert!(read.command.is_some() && read.entrypoint.is_none());
        assert_eq!(read.volumes[1].name.name, "logs");
        assert!(read.volumes[1].owner.is_some());
        assert!(read.mounts[0].readonly.is_some());
        assert_eq!(read.aliases, ["db"]);
        assert_eq!(read.labels[0].0, "team");
        for field in [
            &read.user,
            &read.workdir,
            &read.platform,
            &read.read_only,
            &read.pids_limit,
        ] {
            assert!(field.is_some());
        }
        assert_eq!((read.tmpfs.len(), read.security_opt.len()), (1, 1));
        // context, dockerfile, V, serve, A, /data, /logs, 999, ./conf, /conf,
        // false, /tmp, core, "999", /app, linux/amd64, true,
        // no-new-privileges and 100.
        let mut resolved = 0;
        assert!(
            read.map(&mut |_| {
                resolved += 1;
                Some(())
            })
            .is_some()
        );
        assert_eq!(resolved, 19);
    }
}
