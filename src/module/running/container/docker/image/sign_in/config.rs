//! A sign-in as an account's Docker configuration file gives it, where the
//! `docker` command finds it: from the credential helper the file names for
//! the registry (`credHelpers`), or else from the store it names for every
//! registry (`credsStore`), or else written in the file itself (`auths`).
//! The first of these that the file names is the one asked, as Docker asks
//! it: a helper that has nothing for the registry means no sign-in, whatever
//! the file itself holds.

use std::collections::BTreeMap;
use std::path::Path;

use base64ct::{Base64, Encoding};
use bollard::auth::DockerCredentials;
use serde::Deserialize;

use super::{Environment, Unfound, helper};

/// Docker's configuration file, as far as a sign-in is concerned.
#[derive(Deserialize)]
struct Config {
    auths: Option<BTreeMap<String, Written>>,
    #[serde(rename = "credHelpers")]
    cred_helpers: Option<BTreeMap<String, String>>,
    #[serde(rename = "credsStore")]
    creds_store: Option<String>,
}

/// A sign-in written in the file: a name and a password in `auth`, as
/// `docker login` writes them, or apart, and an identity token in place of
/// a password, or a registry's own token.
#[derive(Deserialize)]
struct Written {
    auth: Option<String>,
    username: Option<String>,
    password: Option<String>,
    identitytoken: Option<String>,
    registrytoken: Option<String>,
}

/// The sign-in that the configuration file at `file` gives for the
/// registry it keeps under `key`; `None` when it gives none, as when there
/// is no file. Helpers are found on the `PATH` that `environment` reads.
pub async fn sign_in(
    file: &Path,
    key: &str,
    environment: Environment<'_>,
) -> Result<Option<DockerCredentials>, Unfound> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(Unfound::Unreadable {
                file: file.to_path_buf(),
                why: error.to_string(),
            });
        }
    };
    // What serde says of a value of the wrong kind quotes the value, which
    // may be a sign-in: only where it is is said.
    let config: Config = serde_json::from_str(&text).map_err(|error| Unfound::NotUnderstood {
        file: file.to_path_buf(),
        line: error.line(),
        column: error.column(),
    })?;
    let named = match config
        .cred_helpers
        .as_ref()
        .and_then(|helpers| helpers.get(key))
    {
        Some(helper) => Some(helper),
        None => config.creds_store.as_ref(),
    };
    match named.filter(|helper| !helper.is_empty()) {
        Some(helper) => helper::ask(helper, key, environment).await,
        None => written(config.auths.unwrap_or_default(), key, file),
    }
}

/// The sign-in for `key` written in the file itself: under that name, or
/// under an address that names the same host, as older `docker login`s
/// wrote it.
fn written(
    mut auths: BTreeMap<String, Written>,
    key: &str,
    file: &Path,
) -> Result<Option<DockerCredentials>, Unfound> {
    let written = match auths.remove(key) {
        Some(written) => Some(written),
        None => (auths.into_iter())
            .find(|(address, _)| host(address) == key)
            .map(|(_, written)| written),
    };
    let Some(written) = written else {
        return Ok(None);
    };
    let (username, password) = match written.auth.filter(|auth| !auth.is_empty()) {
        Some(auth) => {
            let (username, password) = decoded(&auth).ok_or_else(|| Unfound::Undecodable {
                file: file.to_path_buf(),
                key: key.to_string(),
            })?;
            (Some(username), Some(password))
        }
        None => (written.username, written.password),
    };
    let sign_in = DockerCredentials {
        username,
        password,
        identitytoken: written.identitytoken,
        registrytoken: written.registrytoken,
        serveraddress: Some(key.to_string()),
        ..DockerCredentials::default()
    };
    let secret = [
        &sign_in.password,
        &sign_in.identitytoken,
        &sign_in.registrytoken,
    ]
    .into_iter()
    .any(|secret| secret.as_ref().is_some_and(|secret| !secret.is_empty()));
    Ok(secret.then_some(sign_in))
}

/// The name and the password in `auth`, which is `name:password` in
/// base64, as Docker reads it; `None` when it is not that.
fn decoded(auth: &str) -> Option<(String, String)> {
    let bytes = Base64::decode_vec(auth).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let (username, password) = text.split_once(':')?;
    if username.is_empty() {
        return None;
    }
    Some((
        username.to_string(),
        password.trim_matches('\0').to_string(),
    ))
}

/// The host, and port, an address in the file names: what is left of it
/// without its scheme and its path.
fn host(address: &str) -> &str {
    let rest = address.split_once("://").map_or(address, |(_, rest)| rest);
    rest.split('/').next().unwrap_or(rest)
}

#[cfg(test)]
mod tests {
    use super::super::helper::stand_in;
    use super::*;

    /// A configuration file holding `json`, and a folder for helpers.
    struct Place {
        _folder: tempfile::TempDir,
        file: std::path::PathBuf,
        bin: std::path::PathBuf,
    }

    fn place(json: &str) -> Place {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("config.json");
        std::fs::write(&file, json).unwrap();
        let bin = folder.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        Place {
            file,
            bin,
            _folder: folder,
        }
    }

    impl Place {
        async fn sign_in(&self, key: &str) -> Result<Option<DockerCredentials>, Unfound> {
            sign_in(&self.file, key, &stand_in::path(&self.bin)).await
        }
    }

    /// Who signs in, and with what.
    fn who(sign_in: Option<DockerCredentials>) -> Option<(String, String)> {
        sign_in.map(|sign_in| {
            (
                sign_in.username.unwrap_or_default(),
                sign_in.password.unwrap_or_default(),
            )
        })
    }

    fn pair(username: &str, password: &str) -> Option<(String, String)> {
        Some((username.to_string(), password.to_string()))
    }

    #[tokio::test]
    async fn a_sign_in_written_in_the_file_is_read_under_its_registrys_name() {
        let place = place(
            r#"{"auths":{
                "ghcr.io":{"auth":"YnVpbGRlcjpodW50ZXIy"},
                "https://index.docker.io/v1/":{"auth":"aHViLXVzZXI6aHViLXNlY3JldA=="},
                "https://registry.example.test/v2/":{"username":"apart","password":"written"},
                "tokens.example.test":{"identitytoken":"a-token"},
                "empty.example.test":{}
            },"currentContext":"desktop"}"#,
        );
        let sign_in = place.sign_in("ghcr.io").await.unwrap().unwrap();
        assert_eq!(who(Some(sign_in.clone())), pair("builder", "hunter2"));
        assert_eq!(sign_in.serveraddress.as_deref(), Some("ghcr.io"));
        // Docker Hub's is kept under its long name.
        assert_eq!(
            who(place.sign_in("https://index.docker.io/v1/").await.unwrap()),
            pair("hub-user", "hub-secret")
        );
        // An older key, written as an address, names its host.
        assert_eq!(
            who(place.sign_in("registry.example.test").await.unwrap()),
            pair("apart", "written")
        );
        // An identity token goes as one.
        let token = place.sign_in("tokens.example.test").await.unwrap().unwrap();
        assert_eq!(token.identitytoken.as_deref(), Some("a-token"));
        assert_eq!(token.password, None);
        // A registry with nothing written, or not there at all, has none.
        assert!(place.sign_in("empty.example.test").await.unwrap().is_none());
        assert!(place.sign_in("other.example.test").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn the_helper_named_for_the_registry_comes_first_then_the_store_then_the_file() {
        let place = place(
            r#"{
                "auths":{"ghcr.io":{"auth":"ZmlsZS11c2VyOmZpbGUtc2VjcmV0"},"quay.io":{"auth":"ZmlsZS11c2VyOmZpbGUtc2VjcmV0"}},
                "credHelpers":{"ghcr.io":"own","quay.io":""},
                "credsStore":"store"
            }"#,
        );
        stand_in::answering(&place.bin, "own", "own-user");
        stand_in::answering(&place.bin, "store", "store-user");
        assert_eq!(
            who(place.sign_in("ghcr.io").await.unwrap()),
            pair("own-user", "own-user-secret-for-ghcr.io")
        );
        assert_eq!(
            who(place.sign_in("registry.example.test").await.unwrap()),
            pair("store-user", "store-user-secret-for-registry.example.test")
        );
        // A helper named as nothing for a registry leaves it to the file.
        assert_eq!(
            who(place.sign_in("quay.io").await.unwrap()),
            pair("file-user", "file-secret")
        );
        // A helper that has nothing for the registry means no sign-in, as
        // it does to Docker, whatever the file holds.
        stand_in::helper(
            &place.bin,
            "own",
            "echo 'credentials not found in native keychain'; exit 1",
        );
        assert!(place.sign_in("ghcr.io").await.unwrap().is_none());
        // With no store named, the file itself gives it.
        let place = self::place(r#"{"auths":{"ghcr.io":{"auth":"ZmlsZS11c2VyOmZpbGUtc2VjcmV0"}}}"#);
        assert_eq!(
            who(place.sign_in("ghcr.io").await.unwrap()),
            pair("file-user", "file-secret")
        );
    }

    #[tokio::test]
    async fn a_file_that_is_not_there_gives_none_and_one_that_cannot_be_understood_says_where_only()
    {
        let missing = place("{}");
        std::fs::remove_file(&missing.file).unwrap();
        assert!(missing.sign_in("ghcr.io").await.unwrap().is_none());

        let broken = place("{\"auths\": {");
        let unfound = broken.sign_in("ghcr.io").await.unwrap_err();
        assert!(
            matches!(unfound, Unfound::NotUnderstood { line: 1, .. }),
            "{unfound}"
        );

        // A sign-in written where an entry belongs is a value of the wrong
        // kind, which is not quoted.
        let misshapen = place(r#"{"auths":{"ghcr.io":"YnVpbGRlcjpodW50ZXIy"}}"#);
        let said = misshapen.sign_in("ghcr.io").await.unwrap_err().to_string();
        assert!(
            said.ends_with("is not one tay can read, at line 1, column 42"),
            "{said}"
        );
        assert!(!said.contains("YnVpbGRlcjpodW50ZXIy"), "{said}");

        let undecodable = place(r#"{"auths":{"ghcr.io":{"auth":"bm9jb2xvbg=="}}}"#);
        let said = undecodable
            .sign_in("ghcr.io")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            said.starts_with("its sign-in for `ghcr.io` in `")
                && said.ends_with("` is not a name and a password in base64"),
            "{said}"
        );
    }

    #[test]
    fn an_auth_is_a_name_and_a_password_in_base64() {
        assert_eq!(
            decoded("YnVpbGRlcjpodW50ZXIy"),
            Some(("builder".to_string(), "hunter2".to_string()))
        );
        for not in ["bm9jb2xvbg==", "not base64!", "OnNlY3JldA=="] {
            assert_eq!(decoded(not), None, "{not}");
        }
        assert_eq!(host("https://ghcr.io/v2/"), "ghcr.io");
        assert_eq!(host("localhost:5000"), "localhost:5000");
        assert_eq!(host("http://localhost:5000"), "localhost:5000");
    }
}
