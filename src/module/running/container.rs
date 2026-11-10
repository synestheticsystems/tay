//! `container`: an image run through Docker, on the environment's network.

use crate::diagnostic::Diagnostics;
use crate::syntax::fields::{self, Fields};
use crate::syntax::names;
use crate::syntax::yaml::{Kind, Node, Scalar};

use super::common::{self, argv, environment};
use crate::module::shape::{Concern, Place, Published, RestartDefault, Runs, Slot, UnitShape};
use crate::module::want::Want;
use crate::module::{Cx, Module};

pub struct Container;

const FIELDS: &[&str] = &[
    "image",
    "build",
    "command",
    "entrypoint",
    "env",
    "secret_env",
    "volumes",
    "mounts",
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

const SETTING: Place = Place::Readable("a container's settings");

impl Module for Container {
    fn name(&self) -> &'static str {
        "container"
    }

    fn joined_by_entry(&self) -> &'static [&'static str] {
        &["env", "secret_env", "labels", "volumes", "publish"]
    }

    fn check(&self, with: Option<&Node>, cx: &mut Cx<'_>) -> Option<UnitShape> {
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
        let mut slots = Vec::new();
        let mut production = Vec::new();
        match (settings.entry("image"), settings.entry("build")) {
            (Some((_, image)), None) => {
                slots.extend(Slot::read(
                    image,
                    Want::Text,
                    Place::Readable("an image name"),
                    d,
                ));
            }
            (None, Some((key, build))) => {
                slots.extend(build_settings(build, d));
                production.push(Concern::BuildsImage(key.loc));
            }
            (Some(_), Some((key, _))) => d.error(
                key.loc,
                "a `container` unit has exactly one of `image` and `build`; this one has both",
            ),
            (None, None) => d.error(
                settings.loc(),
                "a `container` unit needs exactly one of `image` and `build`",
            ),
        }
        for list in ["command", "entrypoint"] {
            if let Some(node) = settings.get(list) {
                slots.extend(argv(node, &format!("`{list}`"), d).unwrap_or_default());
            }
        }
        slots.extend(environment(&settings, d));
        if let Some(volumes) = settings.get("volumes") {
            slots.extend(volume_settings(volumes, d));
        }
        if let Some(mounts) = settings.get("mounts") {
            let (mount_slots, concerns) = common::mounts(mounts, d);
            slots.extend(mount_slots);
            production.extend(concerns);
        }
        let published = settings
            .get("publish")
            .map(|node| publish_settings(node, &mut slots, d))
            .unwrap_or_default();
        if let Some(aliases) = settings.get("aliases") {
            fields::name_list(aliases, "an alias", names::name_problem, d);
        }
        if let Some(labels) = settings.get("labels") {
            for (_, label) in fields::keyed(labels, "a label", label_key_problem, d) {
                slots.extend(Slot::read(
                    label,
                    Want::Scalar,
                    Place::Readable("a label"),
                    d,
                ));
            }
        }
        for (field, want) in [
            ("user", Want::Scalar),
            ("workdir", Want::Text),
            ("platform", Want::Text),
            ("read_only", Want::Bool),
            ("pids_limit", Want::Number),
        ] {
            if let Some(node) = settings.get(field) {
                slots.extend(Slot::read(node, want, SETTING, d));
            }
        }
        if let Some(options) = settings.get("security_opt")
            && let Some(items) = fields::list(options, "`security_opt`", d)
        {
            for item in items {
                slots.extend(Slot::read(item, Want::Text, SETTING, d));
            }
        }
        (cx.d.len() == errors_before).then_some(UnitShape {
            runs: Runs::OwnContainer { published },
            slots,
            gives: Vec::new(),
            outputs: Vec::new(),
            production,
            ready_without_check: "when its image's own health check passes, or when it has started if the image has none",
            restart_by_default: RestartDefault::ON_FAILURE,
        })
    }
}

fn build_settings(node: &Node, d: &mut Diagnostics) -> Vec<Slot> {
    const BUILD: Place = Place::Readable("an image build");
    let mut slots = Vec::new();
    let Some(build) = Fields::of(node, "`build`", &["context", "dockerfile", "args"], d) else {
        return slots;
    };
    slots.extend(
        build
            .require("context", d)
            .and_then(|n| Slot::read(n, Want::Text, BUILD, d)),
    );
    slots.extend(
        build
            .get("dockerfile")
            .and_then(|n| Slot::read(n, Want::Text, BUILD, d)),
    );
    if let Some(args) = build.get("args") {
        for (_, arg) in fields::keyed(args, "a build argument", names::variable_name_problem, d) {
            slots.extend(Slot::read(
                arg,
                Want::Scalar,
                Place::Readable("an image build argument"),
                d,
            ));
        }
    }
    slots
}

fn volume_settings(node: &Node, d: &mut Diagnostics) -> Vec<Slot> {
    const VOLUME: Place = Place::Readable("a volume");
    let mut slots = Vec::new();
    for (_, volume) in fields::keyed(node, "a volume", names::name_problem, d) {
        if volume.as_map().is_none() {
            slots.extend(Slot::read(volume, Want::Text, VOLUME, d));
            continue;
        }
        let Some(long) = Fields::of(volume, "a volume", &["to", "owner"], d) else {
            continue;
        };
        slots.extend(
            long.require("to", d)
                .and_then(|n| Slot::read(n, Want::Text, VOLUME, d)),
        );
        slots.extend(
            long.get("owner")
                .and_then(|n| Slot::read(n, Want::Number, VOLUME, d)),
        );
    }
    slots
}

/// `publish`: container ports, as keys, each made reachable at a host port.
fn publish_settings(node: &Node, slots: &mut Vec<Slot>, d: &mut Diagnostics) -> Vec<Published> {
    const PUBLISHED: Place = Place::Readable("a published port");
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
        let host_port = if entry.value.as_map().is_some() {
            let Some(long) = Fields::of(&entry.value, "a published port", &["port", "address"], d)
            else {
                continue;
            };
            slots.extend(
                long.get("address")
                    .and_then(|n| Slot::read(n, Want::Text, PUBLISHED, d)),
            );
            long.require("port", d)
        } else {
            Some(&entry.value)
        };
        let Some(host_port) = host_port.and_then(|n| Slot::read(n, Want::PortNumber, PUBLISHED, d))
        else {
            continue;
        };
        published.push(Published {
            container_port,
            host_port: host_port.expr.clone(),
        });
        slots.push(host_port);
    }
    published
}

fn label_key_problem(key: &str) -> Option<String> {
    (key.is_empty() || key.contains(char::is_whitespace) || key.contains("${"))
        .then(|| format!("`{key}` is not a label key: written out, with no spaces"))
}

#[cfg(test)]
mod tests {
    use crate::module::Runs;
    use crate::module::testing::check;

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
        let (shape, errors) = check(
            "container",
            "image: db\npublish:\n  5432: \"${db-port.number}\"\n  8080: { port: 18080, address: 0.0.0.0 }\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
        let Runs::OwnContainer { published } = shape.unwrap().runs else {
            panic!("a container runs in its own container");
        };
        assert_eq!(
            published
                .iter()
                .map(|p| p.container_port)
                .collect::<Vec<_>>(),
            [5432, 8080]
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
    fn a_build_is_a_production_concern_and_volumes_take_two_forms() {
        let (shape, errors) = check(
            "container",
            "build: { context: ., args: { V: 1 } }\nvolumes: { data: /data, logs: { to: /logs, owner: 999 } }\nread_only: yes\n",
        );
        assert_eq!(errors, ["3:12: this must be true or false; this is text"]);
        assert!(shape.is_none());
        let (shape, errors) = check("container", "build: { context: . }\n");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(shape.unwrap().production.len(), 1);
    }
}
