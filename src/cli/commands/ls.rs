//! `tay ls`: the environments on this machine, from the machine's list,
//! each with its folder, whether it is meant to be up, its daemon if one
//! answers, and the ports it keeps. The environment the command is run in
//! is marked.

use std::path::Path;

use serde_json::json;

use crate::cli::Exit;
use crate::cli::args::{Args, Takes, UnitsNamed};
use crate::cli::stack;
use crate::control::client::Client;
use crate::control::protocol;
use crate::definition::read::THIS_TAY;
use crate::disk::{Environment, Intent, Layout, List, Listed, Machine};
use crate::print::out;

use super::plan::print_json;
use super::{block_on, disk_problem, machine, problem};

pub const TAKES: Takes = Takes {
    command: "tay ls",
    choices: false,
    json: true,
    follows: false,
    units: UnitsNamed::None,
};

/// The schema of `--json`. A change to any key or its meaning is a new
/// number.
const SCHEMA: u32 = 1;

const COMMAND: &str = "tay ls";

/// One environment as `tay ls` reports it.
struct Line {
    listed: Listed,
    /// The folder's name, which is the environment's name, for people.
    name: String,
    /// `None` when its folder is missing.
    intent: Option<Intent>,
    daemon: Option<protocol::About>,
    /// What could not be found out about it.
    problems: Vec<String>,
    current: bool,
}

pub fn run(args: Args) -> Exit {
    // Outside any stack there is no environment to mark.
    let current = match &args.root {
        Some(asked) => match stack::root(Some(asked)) {
            Ok(root) => Some(root),
            Err(message) => return problem(COMMAND, Exit::Errors, &message),
        },
        None => stack::root(None).ok(),
    };
    let machine = match machine(COMMAND) {
        Ok(machine) => machine,
        Err(exit) => return exit,
    };
    let listed = match List::known(&machine) {
        Ok(listed) => listed,
        Err(error) => return disk_problem(COMMAND, &error),
    };
    let lines = block_on(COMMAND, async {
        let mut lines = Vec::new();
        for listed in listed {
            let current = current.as_deref() == Some(listed.folder.as_path());
            lines.push(line(listed, &machine, current).await);
        }
        lines
    });
    match lines {
        Ok(lines) => {
            print(&lines, args.json);
            Exit::Done
        }
        Err(exit) => exit,
    }
}

async fn line(listed: Listed, machine: &Machine, current: bool) -> Line {
    let mut problems = Vec::new();
    let layout = Layout::of(&listed.folder);
    let intent = match listed.folder.exists() {
        false => None,
        true => match Intent::read(&layout) {
            Ok(intent) => Some(intent),
            Err(error) => {
                problems.push(error.to_string());
                None
            }
        },
    };
    let environment = Environment {
        layout,
        id: listed.id.clone(),
        machine: machine.clone(),
    };
    let daemon = match Client::find(&environment).await {
        Ok(client) => client.map(|client| client.daemon),
        Err(error) => {
            problems.push(error.to_string());
            None
        }
    };
    Line {
        name: name(&listed.folder),
        listed,
        intent,
        daemon,
        problems,
        current,
    }
}

fn name(folder: &Path) -> String {
    folder
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

fn print(lines: &[Line], json: bool) {
    if json {
        print_json(&json!({
            "schema": SCHEMA,
            "tay": THIS_TAY,
            "environments": lines.iter().map(|line| json!({
                "name": line.name,
                "folder": line.listed.folder,
                "id": line.listed.id.as_str(),
                "folder_exists": line.listed.folder.exists(),
                "intent": line.intent.map(Intent::word),
                "daemon": line.daemon.as_ref().map(|daemon| json!({ "tay": daemon.tay, "pid": daemon.pid })),
                "ports": line.listed.ports,
                "current": line.current,
                "problems": line.problems,
            })).collect::<Vec<_>>(),
        }));
        return;
    }
    if lines.is_empty() {
        out("tay knows no environment on this machine.\n");
        return;
    }
    let names = lines.iter().map(|line| line.name.len()).max().unwrap_or(0);
    let folders = lines
        .iter()
        .map(|line| line.listed.folder.display().to_string().len())
        .max()
        .unwrap_or(0);
    let mut text = String::new();
    for line in lines {
        let mut facts = Vec::new();
        match (line.intent, line.listed.folder.exists()) {
            (Some(intent), _) => facts.push(intent.word().to_string()),
            (None, false) => facts.push("its folder is missing".to_string()),
            (None, true) => {}
        }
        if let Some(daemon) = &line.daemon {
            facts.push(format!("daemon {} (tay {})", daemon.pid, daemon.tay));
        }
        if !line.listed.ports.is_empty() {
            let ports: Vec<String> = line.listed.ports.iter().map(u16::to_string).collect();
            facts.push(format!("ports {}", ports.join(" ")));
        }
        facts.extend(line.problems.iter().cloned());
        text.push_str(&format!(
            "{} {:<names$}  {:<folders$}  {}  {}\n",
            if line.current { "*" } else { " " },
            line.name,
            line.listed.folder.display().to_string(),
            line.listed.id,
            facts.join(", ")
        ));
    }
    out(&text);
}
