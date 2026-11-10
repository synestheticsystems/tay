//! Each command, and what those that reach the daemon share.

pub mod down;
pub mod logs;
pub mod ls;
pub mod plan;
pub mod status;
pub mod up;

use std::future::Future;

use crate::cli::Exit;
use crate::control::client::ClientError;
use crate::disk::{DiskError, Id, List, Machine};
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

/// Says why `command` ends as `exit` says, on standard error.
fn problem(command: &str, exit: Exit, message: &str) -> Exit {
    err(&format!("{command}: {message}\n"));
    exit
}

/// What tay keeps on disk could not be used, so neither can the daemon.
fn disk_problem(command: &str, error: &DiskError) -> Exit {
    problem(command, Exit::Unreachable, &error.to_string())
}

/// The daemon could not do what was asked: another change is under way,
/// or it could not be started or reached.
fn client_problem(command: &str, error: &ClientError) -> Exit {
    let exit = match error {
        ClientError::Refused(_) => Exit::Refused,
        _ => Exit::Unreachable,
    };
    problem(command, exit, &error.to_string())
}
