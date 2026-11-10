//! What a unit's container is created from: its settings as Docker takes
//! them, on the environment's network, with only the published ports the
//! plan opens; and the same by the names of its settings, as the
//! fingerprint of what it was started from takes them.

use std::collections::HashMap;

use serde_json::json;

use bollard::models::{
    ContainerCreateBody, EndpointSettings, HostConfig, Mount, MountType, NetworkingConfig,
    PortBinding,
};

use crate::module::description::OpenedPort;
use crate::module::operations::{self, Failure, Fingerprint, Taking, UnitPlace};
use crate::module::running::command_line::filled_argv;
use crate::module::running::container::{ContainerSettings, Image, host, mounted, volume_name};
use crate::value::Filled;

/// A container's settings, as Docker takes them.
pub struct Creation {
    pub body: ContainerCreateBody,
    pub platform: Option<String>,
    /// The values in its environment that are secret.
    pub secrets: Vec<String>,
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
    let mut secrets = Vec::new();
    for (name, value) in &settings.variables.secret_env {
        env.push(format!("{name}={}", value.reveal()));
        secrets.push(value.reveal());
    }
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
    Ok(Creation {
        body,
        platform,
        secrets,
    })
}

/// What the container of `settings`, for the unit at `place`, opening the
/// ports in `opened`, is started from, by its settings, when it is created
/// from the image `image`, Docker's id for it: the image, not the tag that
/// names it, so that a tag moved to another image makes another container.
pub fn fingerprint(
    settings: &ContainerSettings<Filled>,
    opened: &[OpenedPort<Filled>],
    place: &UnitPlace,
    image: &str,
) -> Fingerprint {
    let mut taking = Taking::of(place);
    taking.setting("image", &image);
    if let Image::Built {
        context,
        dockerfile,
        args,
    } = &settings.image
    {
        let args: Vec<(&String, String)> = args
            .iter()
            .map(|(arg, value)| (arg, value.reveal()))
            .collect();
        let build = json!({
            "context": context.reveal(),
            "dockerfile": dockerfile.as_ref().map(Filled::reveal),
            "args": args,
        });
        taking.setting("build", &build);
    }
    if let Some(command) = &settings.command {
        taking.values("command", command);
    }
    if let Some(entrypoint) = &settings.entrypoint {
        taking.values("entrypoint", entrypoint);
    }
    settings.variables.take(&mut taking);
    if !settings.volumes.is_empty() {
        let volumes: Vec<_> = (settings.volumes.iter())
            .map(|volume| {
                let owner = volume.owner.as_ref().map(Filled::reveal);
                (&volume.name.name, volume.to.reveal(), owner)
            })
            .collect();
        taking.setting("volumes", &volumes);
    }
    if !settings.mounts.is_empty() {
        let mounts: Vec<_> = settings
            .mounts
            .iter()
            .map(|mount| mount.revealed())
            .collect();
        taking.setting("mounts", &mounts);
    }
    if !settings.tmpfs.is_empty() {
        taking.values("tmpfs", &settings.tmpfs);
    }
    if !opened.is_empty() {
        let publish: Vec<_> = (opened.iter())
            .map(|port| {
                let (address, host_port) = (port.address.reveal(), port.host_port.reveal());
                (port.container_port, address, host_port)
            })
            .collect();
        taking.setting("publish", &publish);
    }
    if !settings.aliases.is_empty() {
        taking.setting("aliases", &settings.aliases);
    }
    for (label, value) in &settings.labels {
        taking.value(format!("labels.{label}"), value);
    }
    let each = [
        ("user", &settings.user),
        ("workdir", &settings.workdir),
        ("platform", &settings.platform),
        ("read_only", &settings.read_only),
        ("pids_limit", &settings.pids_limit),
    ];
    for (name, value) in each {
        if let Some(value) = value {
            taking.value(name, value);
        }
    }
    if !settings.security_opt.is_empty() {
        taking.values("security_opt", &settings.security_opt);
    }
    taking.taken()
}
