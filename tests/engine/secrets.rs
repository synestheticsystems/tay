//! `secret` units: generated once, kept in `state/secrets/` for this account
//! alone, and never shown: not in any output, any log, any error, nor in
//! the argument list of anything tay starts.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use crate::harness::Machine;

fn kept(root: &Path, unit: &str) -> serde_json::Value {
    let text = fs::read_to_string(root.join(".tay/state/secrets").join(unit)).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn each_kind_is_generated_as_its_settings_ask() {
    let machine = Machine::new();
    let root = machine.stack("notes");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let password = kept(&root, "notes-password");
    assert_eq!(password["kind"], "password");
    let value = password["value"].as_str().unwrap();
    assert_eq!(value.len(), 32);
    assert!(value.chars().all(|c| c.is_ascii_alphanumeric()));
    // 16 bytes in base64url, with no padding, are 22 characters.
    let key = kept(&root, "notes-key");
    assert_eq!(
        (key["kind"].as_str(), key["encoding"].as_str()),
        (Some("bytes"), Some("base64url"))
    );
    let value = key["value"].as_str().unwrap();
    assert_eq!(value.len(), 22);
    assert!(!value.contains(['=', '+', '/']));
    let signing = kept(&root, "signing");
    assert_eq!(signing["algorithm"], "rsa-2048");
    assert!(
        signing["public"]
            .as_str()
            .unwrap()
            .starts_with("-----BEGIN PUBLIC KEY-----\n")
    );
    assert!(
        signing["private"]
            .as_str()
            .unwrap()
            .starts_with("-----BEGIN PRIVATE KEY-----\n")
    );
    // The file built from the key holds it, and so is for its owner alone.
    let config = root.join(".tay/state/files/config/notes.conf");
    assert!(fs::read_to_string(&config).unwrap().contains(value));
    assert_eq!(
        fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn a_secret_is_generated_once_and_kept_for_this_account_alone() {
    let machine = Machine::new();
    let root = machine.stack("notes");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.last_change("notes-password"), "ready generated");
    let first = kept(&root, "notes-password");
    let again = machine.at(&root, "up", &[]);
    assert_eq!(again.last_change("notes-password"), "ready kept");
    assert_eq!(kept(&root, "notes-password"), first);
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let secrets = root.join(".tay/state/secrets");
    assert_eq!(mode(&secrets), 0o700);
    for unit in ["notes-password", "notes-key", "signing"] {
        assert_eq!(mode(&secrets.join(unit)), 0o600, "{unit}");
    }
}

#[test]
fn an_ed25519_key_pair_fails_saying_tay_cannot_make_one_yet() {
    let machine = Machine::new();
    let root = machine.stack("keypairs");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 1);
    assert_eq!(
        up.last_change("edwards-key"),
        "failed tay cannot make an `ed25519` key pair yet"
    );
    let status = machine.settled(&root);
    assert_eq!(Machine::state(&status, "rsa-key"), "ready kept");
}

#[test]
fn no_secret_appears_in_any_output_log_or_argument_list() {
    let machine = Machine::new();
    let root = machine.stack("notes");
    let mut printed = String::new();
    for args in [
        &["up"][..],
        &["up", "--json"],
        &["status"],
        &["status", "--json"],
        &["ls"],
        &["ls", "--json"],
        &["plan", "--json"],
    ] {
        let run = machine.tay(&root, args);
        printed.push_str(&run.stdout);
        printed.push_str(&run.stderr);
    }
    // Anything the daemon was started with.
    let ps = Command::new("ps")
        .args(["-ax", "-o", "command="])
        .output()
        .unwrap();
    printed.push_str(&String::from_utf8_lossy(&ps.stdout));
    let down = machine.at(&root, "down", &["--json"]);
    printed.push_str(&down.stdout);
    printed.push_str(&fs::read_to_string(root.join(".tay/logs/tay/daemon.log")).unwrap());
    let secrets = [
        kept(&root, "notes-password")["value"].clone(),
        kept(&root, "notes-key")["value"].clone(),
        kept(&root, "signing")["private"].clone(),
    ];
    for secret in secrets {
        let secret = secret.as_str().unwrap();
        // A key's PEM is checked by a line of its body.
        let shown = secret.lines().nth(1).unwrap_or(secret);
        assert!(!printed.contains(shown), "a secret was shown");
    }
}

#[test]
fn a_secret_whose_settings_change_is_kept_saying_so_and_wiping_it_makes_a_new_one() {
    let machine = Machine::new();
    let root = machine.stack("notes");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let first = kept(&root, "notes-password");
    let definitions = fs::read_to_string(root.join("tay.yaml")).unwrap();
    let shorter = definitions.replace("{ kind: password }", "{ kind: password, length: 12 }");
    fs::write(root.join("tay.yaml"), &shorter).unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(
        up.last_change("notes-password"),
        "ready kept, though its settings now ask for another `length`: `tay wipe notes-password --yes` makes a new one"
    );
    assert_eq!(kept(&root, "notes-password"), first);
    let wipe = machine.at(&root, "wipe", &["notes-password", "--yes"]);
    assert_eq!(wipe.code, 0, "{}{}", wipe.stdout, wipe.stderr);
    let made = kept(&root, "notes-password");
    assert_eq!(made["length"], 12);
    assert_eq!(made["value"].as_str().unwrap().len(), 12);
    // One of another kind cannot give what the unit gives, and is kept.
    fs::write(
        root.join("tay.yaml"),
        shorter.replace("{ kind: password, length: 12 }", "{ kind: bytes }"),
    )
    .unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 1);
    assert_eq!(
        up.last_change("notes-password"),
        "failed the one it keeps is a `password`, and its settings now ask for a `bytes`: `tay wipe notes-password --yes` makes a new one"
    );
    assert_eq!(kept(&root, "notes-password"), made);
}

/// Brings up `token` of the stack `secret-ways` under the way `generated`,
/// then under `command`, a way that is a command giving a value of the
/// same name, then under `generated` again, and gives the secret kept at
/// the start and at the end.
fn kept_through(command: &str) -> (serde_json::Value, serde_json::Value) {
    let machine = Machine::new();
    let root = machine.stack("secret-ways");
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.last_change("token"), "ready generated");
    let generated = kept(&root, "token");
    let way = format!("run.default={command}");
    let up = machine.at(&root, "up", &["--set", &way]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(
        up.last_change("token"),
        "ready done: it ran and exited with 0"
    );
    let up = machine.at(&root, "up", &["--set", "run.default=generated"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(up.last_change("token"), "ready kept");
    (generated, kept(&root, "token"))
}

#[test]
fn a_secret_is_kept_through_its_units_way_changing_to_a_command_that_produces_one_and_back() {
    let (start, end) = kept_through("minted");
    assert_eq!(start, end);
}

#[test]
fn a_secret_is_kept_through_its_units_way_changing_to_a_command_that_produces_none_and_back() {
    let (start, end) = kept_through("issued");
    assert_eq!(start, end);
}

#[test]
fn a_secret_kept_in_a_form_tay_cannot_read_is_kept_and_fails_its_unit() {
    let machine = Machine::new();
    let root = machine.stack("notes");
    assert_eq!(machine.at(&root, "up", &[]).code, 0);
    let key = root.join(".tay/state/secrets/notes-key");
    fs::write(&key, "not what tay writes").unwrap();
    let up = machine.at(&root, "up", &[]);
    assert_eq!(up.code, 1);
    assert_eq!(
        up.last_change("notes-key"),
        format!(
            "failed the one it keeps, in `{}`, is not in a form this tay reads",
            key.display()
        )
    );
    assert_eq!(fs::read_to_string(&key).unwrap(), "not what tay writes");
}
