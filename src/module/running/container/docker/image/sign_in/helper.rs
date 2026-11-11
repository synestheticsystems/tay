//! Asking a credential helper for a sign-in, as the `docker` command asks
//! one: a helper or store named `X` is the program `docker-credential-X`,
//! found on the `PATH`, run with `get` and given the registry on its
//! standard input. It answers with the name and the secret in JSON, or, with
//! a failing status, says that it has nothing for the registry.

use std::process::Stdio;
use std::time::Duration;

use bollard::auth::DockerCredentials;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use super::{Environment, Unfound};

/// What a helper says when it has nothing for the registry.
const NOTHING: &str = "credentials not found in native keychain";

/// The name a helper answers with when its secret is an identity token
/// rather than a password.
const TOKEN: &str = "<token>";

/// How long a helper may take to answer. One that waits for a person, as
/// one that unlocks a keychain may, is given time to; one that never
/// answers must not hold the pull for ever.
const ANSWER: Duration = Duration::from_secs(60);

/// The most of what a failing helper said that is told.
const SAID: usize = 200;

/// A helper's answer.
#[derive(Deserialize)]
struct Answer {
    #[serde(rename = "Username", default)]
    username: String,
    #[serde(rename = "Secret", default)]
    secret: String,
}

/// The sign-in the helper or store named `name` gives for the registry it
/// keeps under `key`; `None` when it has none. The helper is found on the
/// `PATH` that `environment` reads.
pub async fn ask(
    name: &str,
    key: &str,
    environment: Environment<'_>,
) -> Result<Option<DockerCredentials>, Unfound> {
    let program = format!("docker-credential-{name}");
    let failed = |why: String| Unfound::HelperFailed {
        program: program.clone(),
        why,
    };
    let mut command = tokio::process::Command::new(&program);
    command
        .arg("get")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = environment("PATH") {
        command.env("PATH", path);
    }
    let mut child = command.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => Unfound::HelperMissing {
            program: program.clone(),
        },
        _ => failed(format!("cannot be started: {error}")),
    })?;
    let stdin = child.stdin.take();
    let asked = async move {
        if let Some(mut stdin) = stdin {
            // A helper that ends without reading what it is asked has
            // answered all the same: what it wrote and how it ended say
            // what, so its closed input is no failure to ask it.
            match stdin.write_all(key.as_bytes()).await {
                Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => return Err(error),
                _ => {}
            }
        }
        child.wait_with_output().await
    };
    let output = tokio::time::timeout(ANSWER, asked)
        .await
        .map_err(|_| {
            failed(format!(
                "did not answer within {} seconds",
                ANSWER.as_secs()
            ))
        })?
        .map_err(|error| failed(format!("could not be asked: {error}")))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stdout);
        let said = said.trim();
        if said == NOTHING {
            return Ok(None);
        }
        let said: String = said.chars().take(SAID).collect();
        return Err(failed(format!("ended with {}: {said}", output.status)));
    }
    // What serde says of an answer it cannot read may quote it, and so the
    // secret: it is not said.
    let answer: Answer = serde_json::from_slice(&output.stdout)
        .map_err(|_| failed("answered with something that is not a sign-in".to_string()))?;
    if answer.secret.is_empty() {
        return Ok(None);
    }
    let mut sign_in = DockerCredentials {
        serveraddress: Some(key.to_string()),
        ..DockerCredentials::default()
    };
    match answer.username == TOKEN {
        true => sign_in.identitytoken = Some(answer.secret),
        false => {
            sign_in.username = Some(answer.username);
            sign_in.password = Some(answer.secret);
        }
    }
    Ok(Some(sign_in))
}

/// Credential helpers that stand in for real ones in tests: shell scripts
/// on a `PATH` of their own.
#[cfg(test)]
pub mod stand_in {
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    /// Writes the helper `docker-credential-NAME` into `bin`: a shell
    /// script that runs `body`.
    pub fn helper(bin: &Path, name: &str, body: &str) {
        let path = bin.join(format!("docker-credential-{name}"));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Writes the helper `NAME`, which answers `get` with the name `user`
    /// and a secret that names the registry it was given, and fails
    /// anything else.
    pub fn answering(bin: &Path, name: &str, user: &str) {
        helper(
            bin,
            name,
            &format!(
                r#"[ "$1" = get ] || exit 3
registry=$(cat)
printf '{{"ServerURL":"%s","Username":"{user}","Secret":"{user}-secret-for-%s"}}' "$registry" "$registry""#
            ),
        );
    }

    /// The environment of a process whose `PATH` is `bin` and the system's
    /// own folders, and nothing else.
    pub fn path(bin: &Path) -> impl Fn(&str) -> Option<OsString> + Sync + use<> {
        let mut path = bin.as_os_str().to_owned();
        path.push(":/usr/bin:/bin");
        move |name| (name == "PATH").then(|| path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GHCR: &str = "ghcr.io";

    fn bin() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[tokio::test]
    async fn a_helper_is_asked_with_get_and_the_registry_on_its_input() {
        let bin = bin();
        stand_in::answering(bin.path(), "own", "builder");
        let sign_in = ask("own", GHCR, &stand_in::path(bin.path()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sign_in.username.as_deref(), Some("builder"));
        assert_eq!(
            sign_in.password.as_deref(),
            Some("builder-secret-for-ghcr.io")
        );
        assert_eq!(sign_in.serveraddress.as_deref(), Some(GHCR));
        assert_eq!(sign_in.identitytoken, None);
    }

    #[tokio::test]
    async fn a_helpers_token_goes_as_an_identity_token() {
        let bin = bin();
        stand_in::answering(bin.path(), "tokens", TOKEN);
        let sign_in = ask("tokens", GHCR, &stand_in::path(bin.path()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            sign_in.identitytoken.as_deref(),
            Some("<token>-secret-for-ghcr.io")
        );
        assert_eq!(sign_in.username, None);
        assert_eq!(sign_in.password, None);
    }

    #[tokio::test]
    async fn a_helper_with_nothing_for_the_registry_gives_none() {
        let bin = bin();
        stand_in::helper(bin.path(), "empty", &format!("echo '{NOTHING}'; exit 1"));
        assert!(
            ask("empty", GHCR, &stand_in::path(bin.path()))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_helper_that_ends_without_reading_what_it_is_asked_has_answered_all_the_same() {
        let bin = bin();
        stand_in::helper(bin.path(), "empty", &format!("echo '{NOTHING}'; exit 1"));
        // More than a pipe holds, so that writing it cannot end before the
        // helper has, and meets its closed input every time.
        let registry = "r".repeat(1 << 20);
        assert!(
            ask("empty", &registry, &stand_in::path(bin.path()))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_helper_that_is_missing_fails_or_answers_nonsense_is_said_without_its_answer() {
        let bin = bin();
        let path = stand_in::path(bin.path());
        let said = ask("gone", GHCR, &path).await.unwrap_err().to_string();
        assert_eq!(
            said,
            "the credential helper it names, `docker-credential-gone`, is not installed"
        );

        stand_in::helper(
            bin.path(),
            "locked",
            "echo 'the keychain is locked'; exit 1",
        );
        let said = ask("locked", GHCR, &path).await.unwrap_err().to_string();
        assert!(
            said.starts_with(
                "the credential helper it names, `docker-credential-locked`, ended with "
            ) && said.ends_with(": the keychain is locked"),
            "{said}"
        );

        stand_in::helper(bin.path(), "garbled", "echo 'Secret: hunter2'");
        let said = ask("garbled", GHCR, &path).await.unwrap_err().to_string();
        assert_eq!(
            said,
            "the credential helper it names, `docker-credential-garbled`, answered with something that is not a sign-in"
        );
    }
}
