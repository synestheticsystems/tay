//! Containers, through the real binary against this machine's Docker, with
//! small images: what belongs to an environment and is never touched
//! otherwise, bringing a container up from its settings and judging it
//! ready, how it ended and what follows, finding it again, its logs,
//! replacing it, taking it down, commands run in one, wiping what it keeps,
//! the host and its containers reaching each other, actions that use one,
//! and Docker itself stopping under them.

mod actions;
mod bringing_up;
mod commands;
mod ending;
mod environment;
mod finding_again;
mod logs;
mod reaching;
mod readiness;
mod secrets;
mod when_docker_stops;
mod wiping;

use std::path::Path;

use serde_json::Value;

use crate::docker;
use crate::harness::Machine;

/// The name of the container of `unit` in the environment at `root`.
pub fn name(machine: &Machine, root: &Path, unit: &str) -> String {
    format!("tay-{}-{unit}", machine.id(root))
}

/// What Docker says of the container of `unit` in the environment at
/// `root`; `None` when there is none.
pub fn container(machine: &Machine, root: &Path, unit: &str) -> Option<Value> {
    docker::inspect(&name(machine, root, unit))
}

/// The names of every container, volume and network, in that order, that
/// carries the label of the environment at `root`.
pub fn labelled(machine: &Machine, root: &Path) -> [Vec<String>; 3] {
    let label = format!("label=tay.environment={}", machine.id(root));
    let names = |args: &[&str]| -> Vec<String> {
        let mut names: Vec<String> = docker::docker(args)
            .split_whitespace()
            .map(str::to_string)
            .collect();
        names.sort();
        names
    };
    [
        names(&["ps", "-a", "--format", "{{.Names}}", "--filter", &label]),
        names(&["volume", "ls", "--format", "{{.Name}}", "--filter", &label]),
        names(&["network", "ls", "--format", "{{.Name}}", "--filter", &label]),
    ]
}
