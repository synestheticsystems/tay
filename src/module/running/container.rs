//! `container`: an image run through Docker, on the environment's network.

mod read;

use std::sync::Arc;

use crate::syntax::names::NameAt;
use crate::syntax::yaml::Node;
use crate::value::Filled;

use super::NotRunnable;
use super::mounts::{self, Mount};
use super::variables::Variables;
use crate::module::description::{
    Concern, Description, ReadyWithoutCheck, RestartOn, Runs, UnitKind,
};
use crate::module::operations::Operations;
use crate::module::settings::{MapValues, each, each_named, maybe};
use crate::module::{Cx, Module};

pub struct Container;

/// The name the host has inside the environment's containers, which Docker
/// gives it.
pub const HOST_IN_CONTAINERS: &str = "host.docker.internal";

/// `${host.listen}`: the address a host process listens on so that this
/// machine's containers can reach it and other machines cannot. On macOS,
/// Docker Desktop brings a container's connections to the host's own
/// address.
#[cfg(target_os = "macos")]
pub fn host_listen() -> Result<&'static str, &'static str> {
    Ok("127.0.0.1")
}

/// `${host.listen}` elsewhere: the host's address on the environment's
/// network, which is known once the network exists.
#[cfg(not(target_os = "macos"))]
pub fn host_listen() -> Result<&'static str, &'static str> {
    Err(
        "on this system it is the host's address on the environment's network, and tay does not make networks yet",
    )
}

/// A container's settings, each value a [`Slot`](crate::module::Slot) as written or a [`Value`](crate::value::Value)
/// once resolved. Its published ports are in the unit's description, where
/// the planner decides which are opened.
#[derive(Debug)]
struct ContainerSettings<V> {
    image: Image<V>,
    /// `None`: the image's own.
    command: Option<Vec<V>>,
    /// `None`: the image's own. Empty: none.
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

impl<V> MapValues for ContainerSettings<V> {
    type Value = V;
    type With<W> = ContainerSettings<W>;

    fn map_values<'a, W>(
        &'a self,
        f: &mut dyn FnMut(&'a V) -> Option<W>,
    ) -> Option<ContainerSettings<W>> {
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

/// tay cannot run such a unit yet, so its operations fail, saying so, and
/// what its settings say is not used.
impl From<ContainerSettings<Filled>> for Operations {
    fn from(_: ContainerSettings<Filled>) -> Operations {
        Operations::Running(Arc::new(NotRunnable {
            what: "a `container` unit",
        }))
    }
}

impl Module for Container {
    fn name(&self) -> &'static str {
        "container"
    }

    fn joined_by_entry(&self) -> &'static [&'static str] {
        &["env", "secret_env", "labels", "volumes", "publish"]
    }

    fn read(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<Description> {
        let read::Read {
            settings,
            publish,
            built_at,
        } = read::read(with, cx)?;
        let volumes = settings
            .volumes
            .iter()
            .map(|volume| volume.name.clone())
            .collect();
        let (volume_mounts, mut production) = mounts::described(&settings.mounts);
        production.extend(built_at.map(Concern::BuildsImage));
        Some(Description {
            publish,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::settings::Slot;
    use crate::module::testing::{self, check};
    use crate::module::want::{PathUse, Want};
    use read::read;

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
                .publish
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
    fn an_empty_entrypoint_clears_the_images_own_and_an_empty_command_is_an_error() {
        let read = settings("image: app\nentrypoint: []\n");
        assert!(read.entrypoint.is_some_and(|words| words.is_empty()));
        let (_, errors) = check("container", "image: app\ncommand: []\n");
        assert_eq!(errors, ["2:10: `command` must name a program"]);
    }

    #[test]
    fn each_setting_is_read_into_its_own_field_and_every_value_is_resolved() {
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
            read.map_values(&mut |_| {
                resolved += 1;
                Some(())
            })
            .is_some()
        );
        assert_eq!(resolved, 19);
    }
}
