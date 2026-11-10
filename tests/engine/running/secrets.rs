//! Secrets reach a program only through its environment: never an
//! argument, never in a record, never in a line tay writes, never in what a
//! command prints.

use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::harness::{Machine, child, processes};

/// Every file under `folder`, with its text.
fn every_file(folder: &Path, files: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            every_file(&path, files);
        } else {
            let text = String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned();
            files.push((path.display().to_string(), text));
        }
    }
}

fn kept(root: &Path, file: &str, name: &str) -> String {
    let text = fs::read_to_string(root.join(".tay/state/secrets").join(file)).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json[name].as_str().unwrap().to_string()
}

#[test]
fn no_secret_appears_in_any_output_log_record_or_argument_list() {
    let machine = Machine::new();
    let root = machine.stack("secret-run");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let password = kept(&root, "db-password", "value");
    let token = kept(&root, "issuer", "token");
    // Each reached the program, through its environment.
    let log = crate::harness::log(&root, "server");
    assert!(
        log.contains(&format!(
            "| has its password of {} characters and a token of {}",
            password.len(),
            token.len()
        )),
        "{log}"
    );
    let mut said = vec![
        ("up".to_string(), up.stdout.clone() + &up.stderr),
        (
            "up --json".to_string(),
            machine.at(&root, "up", &["--json"]).stdout,
        ),
    ];
    for args in [
        &["status"][..],
        &["status", "--json"],
        &["logs", "server"],
        &["logs", "issuer"],
    ] {
        let run = machine.at(&root, args[0], &args[1..]);
        said.push((args.join(" "), run.stdout + &run.stderr));
    }
    // The argument list of every process of the environment, the keepers'
    // and the programs' among them.
    let machine_processes = machine.processes();
    let arguments: Vec<String> = processes()
        .into_iter()
        .filter(|(pid, _, _)| machine_processes.contains(pid))
        .map(|(_, _, command)| command)
        .collect();
    assert!(arguments.iter().any(|command| command.contains("tay keep")));
    said.push(("argument lists".to_string(), arguments.join("\n")));
    // A program killed and started again, and the environment taken down.
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(child(&root, "server")).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    machine.until_state(&root, "server", Duration::from_secs(15), |state| {
        state.starts_with("ready attempt 2")
    });
    let down = machine.at(&root, "down", &["--json"]);
    said.push(("down".to_string(), down.stdout + &down.stderr));
    // Everything tay keeps of the environment but the secrets themselves.
    let mut files = Vec::new();
    every_file(&root.join(".tay"), &mut files);
    files.retain(|(path, _)| !path.contains("/.tay/state/secrets/"));
    assert!(
        files
            .iter()
            .any(|(path, _)| path.ends_with("/units/server/record.json"))
    );
    said.extend(files);
    for (what, text) in said {
        for secret in [&password, &token] {
            assert!(
                !text.contains(secret.as_str()),
                "a secret is in {what}:\n{text}"
            );
        }
    }
}
