//! What a unit's container is created from: its settings as Docker takes
//! them, on the environment's network, with only the published ports the
//! plan opens; and what tells whether a container that exists was created
//! from the same, which leaves out every secret. The secrets in its
//! environment are compared with the container's own, which Docker shows
//! to whoever may ask it, and never written anywhere else.

use std::collections::HashMap;

use serde_json::json;

use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, EndpointSettings, HostConfig, Mount, MountType,
    NetworkingConfig, PortBinding,
};

use crate::module::description::OpenedPort;
use crate::module::operations::{self, Failure, UnitPlace};
use crate::module::running::command_line::filled_argv;
use crate::module::running::container::docker::ours::SETTINGS;
use crate::module::running::container::{ContainerSettings, host, mounted, volume_name};
use crate::value::Filled;

/// A container's settings, as Docker takes them.
pub struct Creation {
    pub body: ContainerCreateBody,
    pub platform: Option<String>,
    /// The values in its environment that are secret.
    pub secrets: Vec<String>,
    /// Its secret variables as its environment holds them, `NAME=value`.
    secret_env: Vec<String>,
    /// Everything it is created from but its secrets, with the image it
    /// runs as Docker names it: what its label keeps.
    pub settings: String,
}

/// The image a container is created from, as it is here: by the name its
/// settings give, or the tag tay builds it as, and its id.
pub struct ImageHere {
    pub name: String,
    pub id: String,
}

/// The container of `settings`, for the unit at `place`, from `image`, on
/// the environment's network, opening the ports in `opened`. `gateway` is
/// the host's address on the network, which on Linux is what the host's
/// name means inside it.
pub fn creation(
    settings: &ContainerSettings<Filled>,
    opened: &[OpenedPort<Filled>],
    place: &UnitPlace,
    image: &ImageHere,
    gateway: Option<&str>,
) -> Result<Creation, Failure> {
    let network = place.environment.network();
    let mut env = Vec::new();
    for (name, value) in &settings.variables.env {
        env.push(format!("{name}={}", value.reveal()));
    }
    let secret_env: Vec<String> = settings
        .variables
        .secret_env
        .iter()
        .map(|(name, value)| format!("{name}={}", value.reveal()))
        .collect();
    let secrets = settings
        .variables
        .secret_env
        .iter()
        .map(|(_, value)| value.reveal())
        .collect();
    env.extend(secret_env.iter().cloned());
    let mut mounts = Vec::new();
    for volume in &settings.volumes {
        mounts.push(Mount {
            typ: Some(MountType::VOLUME),
            source: Some(volume_name(place, &place.unit, &volume.name.name)),
            target: Some(operations::text(&volume.to, "volumes")?.to_string()),
            ..Mount::default()
        });
    }
    for mount in &settings.mounts {
        mounts.push(mounted(mount, place)?);
    }
    let tmpfs: HashMap<String, String> = settings
        .tmpfs
        .iter()
        .map(|path| Ok((operations::text(path, "tmpfs")?.to_string(), String::new())))
        .collect::<Result<_, Failure>>()?;
    let mut port_bindings = HashMap::new();
    let mut exposed_ports = Vec::new();
    for port in opened {
        let key = format!("{}/tcp", port.container_port);
        exposed_ports.push(key.clone());
        port_bindings.insert(
            key,
            Some(vec![PortBinding {
                host_ip: Some(operations::text(&port.address, "publish")?.to_string()),
                host_port: Some(operations::number(&port.host_port, "publish")?.to_string()),
            }]),
        );
    }
    let mut aliases = vec![place.unit.clone()];
    aliases.extend(settings.aliases.iter().cloned());
    let labels: HashMap<String, String> = settings
        .labels
        .iter()
        .map(|(key, value)| (key.clone(), value.reveal()))
        .collect();
    let host_config = HostConfig {
        mounts: Some(mounts),
        tmpfs: (!tmpfs.is_empty()).then_some(tmpfs),
        port_bindings: (!port_bindings.is_empty()).then_some(port_bindings),
        network_mode: Some(network.clone()),
        extra_hosts: host::host_entries(gateway),
        readonly_rootfs: settings
            .read_only
            .as_ref()
            .map(|read_only| operations::boolean(read_only, "read_only"))
            .transpose()?,
        security_opt: (!settings.security_opt.is_empty())
            .then(|| {
                settings
                    .security_opt
                    .iter()
                    .map(|option| Ok(operations::text(option, "security_opt")?.to_string()))
                    .collect::<Result<Vec<_>, Failure>>()
            })
            .transpose()?,
        pids_limit: settings
            .pids_limit
            .as_ref()
            .map(|limit| operations::number(limit, "pids_limit"))
            .transpose()?,
        ..HostConfig::default()
    };
    let body = ContainerCreateBody {
        image: Some(image.name.clone()),
        cmd: settings
            .command
            .as_ref()
            .map(|words| filled_argv(words, "`command`"))
            .transpose()?,
        // An empty entrypoint clears the image's own, which Docker is told
        // by one empty word.
        entrypoint: settings
            .entrypoint
            .as_ref()
            .map(|words| match words.is_empty() {
                true => Ok(vec![String::new()]),
                false => filled_argv(words, "`entrypoint`"),
            })
            .transpose()?,
        env: Some(env),
        user: settings.user.as_ref().map(Filled::reveal),
        working_dir: settings
            .workdir
            .as_ref()
            .map(|workdir| Ok::<_, Failure>(operations::text(workdir, "workdir")?.to_string()))
            .transpose()?,
        exposed_ports: (!exposed_ports.is_empty()).then_some(exposed_ports),
        labels: Some(labels),
        host_config: Some(host_config),
        networking_config: Some(NetworkingConfig {
            endpoints_config: Some(HashMap::from([(
                network,
                EndpointSettings {
                    aliases: Some(aliases),
                    ..EndpointSettings::default()
                },
            )])),
        }),
        ..ContainerCreateBody::default()
    };
    let platform = settings
        .platform
        .as_ref()
        .map(|platform| Ok::<_, Failure>(operations::text(platform, "platform")?.to_string()))
        .transpose()?;
    let settings = fingerprint(&body, &secret_env, platform.as_deref(), &image.id);
    Ok(Creation {
        body,
        platform,
        secrets,
        secret_env,
        settings,
    })
}

impl Creation {
    /// Whether the container Docker says `inspected` of was created from
    /// these settings and the image that is now: its label keeps the rest,
    /// and its environment holds each secret.
    pub fn created_from(&self, inspected: &ContainerInspectResponse) -> bool {
        let Some(config) = &inspected.config else {
            return false;
        };
        let labelled = config
            .labels
            .as_ref()
            .and_then(|labels| labels.get(SETTINGS))
            .is_some_and(|kept| *kept == self.settings);
        let env = config.env.as_deref().unwrap_or_default();
        labelled && self.secret_env.iter().all(|secret| env.contains(secret))
    }
}

/// What a container's label keeps of its settings: all of them but the
/// values of its secrets, whose names alone are kept, in an order that
/// does not depend on how they were gathered.
fn fingerprint(
    body: &ContainerCreateBody,
    secret_env: &[String],
    platform: Option<&str>,
    image: &str,
) -> String {
    let mut body = body.clone();
    body.env = body.env.map(|env| {
        env.into_iter()
            .filter(|entry| !secret_env.contains(entry))
            .collect()
    });
    let secret_names: Vec<&str> = secret_env
        .iter()
        .filter_map(|entry| entry.split_once('=').map(|(name, _)| name))
        .collect();
    // A JSON object keeps its keys in order, so a mapping gathered in any
    // order is written alike.
    json!({
        "image": image,
        "platform": platform,
        "secret_env": secret_names,
        "create": body,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use bollard::models::ContainerConfig;

    use super::*;

    /// A container's body, with `labels`, and `secret` among its
    /// environment as `secret_env` puts it there.
    fn body(labels: &[(&str, &str)], secret: &str) -> ContainerCreateBody {
        ContainerCreateBody {
            image: Some("busybox:1.36".to_string()),
            env: Some(vec!["MODE=fast".to_string(), secret.to_string()]),
            labels: Some(
                labels
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect(),
            ),
            ..ContainerCreateBody::default()
        }
    }

    fn kept(labels: &[(&str, &str)], secret: &str, image: &str) -> String {
        fingerprint(&body(labels, secret), &[secret.to_string()], None, image)
    }

    #[test]
    fn what_a_container_is_created_from_is_kept_without_its_secrets_and_in_one_order() {
        let one = kept(
            &[("a", "1"), ("b", "2"), ("c", "3")],
            "TOKEN=hunter2",
            "sha256:1",
        );
        let other = kept(
            &[("c", "3"), ("a", "1"), ("b", "2")],
            "TOKEN=hunter2",
            "sha256:1",
        );
        assert_eq!(one, other);
        assert!(!one.contains("hunter2"), "{one}");
        assert!(one.contains("TOKEN") && one.contains("MODE=fast"), "{one}");
        // Another image, or another secret's name, is another container;
        // another secret's value is told by the container's environment.
        let labels = &[("a", "1"), ("b", "2"), ("c", "3")];
        assert_ne!(one, kept(labels, "TOKEN=hunter2", "sha256:2"));
        assert_ne!(one, kept(labels, "KEY=hunter2", "sha256:1"));
        assert_eq!(one, kept(labels, "TOKEN=other", "sha256:1"));
    }

    #[test]
    fn a_container_was_created_from_these_settings_when_its_label_and_its_secrets_are_theirs() {
        let creation = Creation {
            body: body(&[], "TOKEN=hunter2"),
            platform: None,
            secrets: vec!["hunter2".to_string()],
            secret_env: vec!["TOKEN=hunter2".to_string()],
            settings: kept(&[], "TOKEN=hunter2", "sha256:1"),
        };
        let inspected = |settings: &str, env: &[&str]| ContainerInspectResponse {
            config: Some(ContainerConfig {
                labels: Some([(SETTINGS.to_string(), settings.to_string())].into()),
                env: Some(env.iter().map(|entry| entry.to_string()).collect()),
                ..ContainerConfig::default()
            }),
            ..ContainerInspectResponse::default()
        };
        let settings = creation.settings.clone();
        assert!(creation.created_from(&inspected(&settings, &["PATH=/bin", "TOKEN=hunter2"])));
        assert!(!creation.created_from(&inspected(&settings, &["TOKEN=other"])));
        assert!(!creation.created_from(&inspected("{}", &["TOKEN=hunter2"])));
    }
}
