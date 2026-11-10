//! Each command, and what those that reach the daemon share.

pub mod down;
pub mod logs;
pub mod ls;
pub mod plan;
pub mod restart;
pub mod set;
pub mod status;
pub mod unset;
pub mod up;

use std::future::Future;

use crate::cli::Exit;
use crate::control::client::{Client, ClientError};
use crate::control::protocol::Refused;
use crate::disk::{DiskError, Environment, Id, Intent, List, Machine};
use crate::module;
use crate::print::err;

/// Runs `work` to its end, for a command that talks to the daemon.
fn block_on<F: Future>(command: &str, work: F) -> Result<F::Output, Exit> {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => Ok(runtime.block_on(work)),
        Err(error) => Err(problem(
            command,
            Exit::Unreachable,
            &format!("tay cannot start its runtime: {error}"),
        )),
    }
}

/// This machine's folders for tay.
fn machine(command: &str) -> Result<Machine, Exit> {
    Machine::from_env().map_err(|error| disk_problem(command, &error))
}

/// Takes off the machine's list each environment of which nothing is left:
/// no folder, no daemon, and nothing where its units run. Gives each that
/// stays because whether anything of it is left where its units run cannot
/// be told, with why.
async fn forget_what_is_gone(machine: &Machine) -> Result<Vec<(Id, String)>, DiskError> {
    let mut gone = Vec::new();
    let mut untold = Vec::new();
    for listed in List::without_folder_or_daemon(machine)? {
        match module::anything_left_of_environment(&listed).await {
            Ok(false) => gone.push(listed.id),
            Ok(true) => {}
            Err(why) => untold.push((listed.id, why.to_string())),
        }
    }
    List::forget(machine, &gone)?;
    Ok(untold)
}

/// The daemon of `environment`, for a change that brings units up: the one
/// that answers, or one started when the environment is meant to be up.
/// With none running and the environment down, the change is refused as
/// the daemon would refuse it, and no daemon is started for it.
async fn daemon_for_up(command: &str, environment: &Environment) -> Result<Client, Exit> {
    let none = match Client::find(environment).await {
        Ok(found) => found.is_none(),
        Err(error) => return Err(client_problem(command, &error)),
    };
    if none {
        match Intent::read(&environment.layout) {
            Ok(Intent::Up) => {}
            Ok(Intent::Down) => {
                let refused = ClientError::Refused(Refused::environment_down());
                return Err(client_problem(command, &refused));
            }
            Err(error) => return Err(disk_problem(command, &error)),
        }
    }
    Client::ensure(environment)
        .await
        .map_err(|error| client_problem(command, &error))
}

/// Says why `command` ends as `exit` says, on standard error.
fn problem(command: &str, exit: Exit, message: &str) -> Exit {
    err(&format!("{command}: {message}\n"));
    exit
}

/// What tay keeps on disk could not be used, so neither can the daemon.
fn disk_problem(command: &str, error: &DiskError) -> Exit {
    problem(command, Exit::Unreachable, &error.to_string())
}

/// The daemon could not do what was asked: it was refused as things
/// stand, it does not fit the stack, or the daemon could not be started or
/// reached.
fn client_problem(command: &str, error: &ClientError) -> Exit {
    let exit = match error {
        ClientError::Refused(_) => Exit::Refused,
        ClientError::DoesNotFit(_) => Exit::Errors,
        _ => Exit::Unreachable,
    };
    problem(command, exit, &error.to_string())
}
