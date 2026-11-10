//! The modules whose units run something: a process on the host, a container,
//! and a command run once; the settings they share, each module still
//! deciding which of these it has; and what is found of a unit where it runs
//! whatever its settings.

pub mod command;
pub mod command_line;
pub mod container;
mod host;
mod mounts;
pub mod process;
mod variables;

use super::description::Runs;
use super::operations::{Failure, UnitPlace};
use crate::disk::{DiskError, Environment, Listed};

/// What a unit that runs keeps of itself, as a person reads it: its record,
/// with its lock and what it noted of how its steps ended, in its folder of
/// the state folder.
const RECORD: &str = "its record";

/// Removes what a unit that runs keeps of itself: its folder in the state
/// folder. It is stopped, so nothing holds its lock.
fn remove_record(place: &UnitPlace) -> Result<(), Failure> {
    let folder = place.environment.layout.unit_state(&place.unit);
    match std::fs::remove_dir_all(&folder) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DiskError::io(folder, "removed")(error).into()),
    }
}

/// Whether anything of a unit that runs where `runs` says is left now, from
/// its place alone, whatever its settings: a process of it that runs, or a
/// container of it, running or ended, which is the environment's to
/// remove. What `tay down` asks to check that nothing is left, and a daemon
/// asks of a unit whose settings it cannot fill.
pub async fn anything_left(runs: &Runs, place: &UnitPlace) -> Result<bool, Failure> {
    match runs {
        Runs::OnHost => host::running_now(place),
        Runs::OwnContainer | Runs::OneOffContainer => container::anything_left(place).await,
        // A command run inside another unit's container leaves nothing of
        // its own: what it ran in is that unit's.
        Runs::InsideUnit(_) => Ok(false),
    }
}

/// The units of `environment` of which anything is left, found where units
/// run, whether or not the plan has them, and where each runs: what
/// `tay down` stops beside the units it knows, what a daemon reports of the
/// units its plan does not have, and what a command asks before it says
/// that nothing runs where no daemon does.
pub async fn found_left(environment: &Environment) -> Result<Vec<(String, Runs)>, Failure> {
    let mut found: Vec<(String, Runs)> = host::found_running(&environment.layout)?
        .into_iter()
        .map(|unit| (unit, Runs::OnHost))
        .collect();
    found.extend(container::found_left(environment).await?);
    Ok(found)
}

/// Stops what of a unit that runs where `runs` says is left, and whatever
/// it started in turn, from its place alone, whatever its settings, and
/// waits until nothing of it is left.
pub async fn stop(runs: &Runs, place: &UnitPlace) -> Result<(), Failure> {
    match runs {
        Runs::OnHost => {
            let place = place.clone();
            tokio::task::spawn_blocking(move || host::stop(&place))
                .await
                .map_err(|error| host::panicked("stopping it", &error))?
        }
        Runs::OwnContainer | Runs::OneOffContainer => container::stop(place).await,
        // Docker can stop no program run inside a container but by stopping
        // the container, which is another unit's: such a command goes on
        // until it ends, or that container stops.
        Runs::InsideUnit(_) => Ok(()),
    }
}

/// Removes what the environment's units share where they run, once each
/// of them is stopped: its network. Its volumes are kept: they hold its
/// data.
pub async fn remove_shared(environment: &Environment) -> Result<(), SharedLeft> {
    container::remove_shared(environment)
        .await
        .map_err(|why| SharedLeft {
            what: environment.network(),
            why,
        })
}

/// What the environment's units share that could not be removed, by its
/// name, and why.
#[derive(Debug)]
pub struct SharedLeft {
    pub what: String,
    pub why: Failure,
}

/// What the environment keeps where its units run, beyond its folder, as a
/// person reads each: its volumes, and the images built for it.
pub async fn kept_by_environment(environment: &Environment) -> Result<Vec<String>, Failure> {
    container::kept_by_environment(environment).await
}

/// Removes what the environment keeps where its units run, once every
/// unit of it is stopped: its volumes, and the images built for it. Gives
/// what was removed, as a person reads each.
pub async fn remove_kept_by_environment(environment: &Environment) -> Result<Vec<String>, Failure> {
    container::remove_kept_by_environment(environment).await
}

/// Whether anything of the environment `listed` is left where its units
/// run that its folder cannot tell: a container, a volume or a network of
/// it.
pub async fn anything_left_of_environment(listed: &Listed) -> Result<bool, Failure> {
    container::anything_left_of_environment(listed).await
}
