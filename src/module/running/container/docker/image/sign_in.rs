//! The sign-in a pull is sent with. Docker's engine keeps none: whoever asks
//! it to pull sends the registry's sign-in with the request, and the
//! `docker` command sends the one its account gave it with `docker login`.
//! tay does the same for the account it runs as, reading that account's
//! Docker configuration as the `docker` command reads it ([`config`]).
//! Nothing of a sign-in is printed, logged, kept or put on a command line:
//! it is read when a pull begins, and dropped when the pull ends.
//!
//! A configuration that cannot be read, or a credential helper that fails,
//! is no failure of the unit: the pull goes without a sign-in, and if
//! Docker then refuses it, the refusal says both. A refusal of access says
//! which registry it was, which account's configuration was looked in, and
//! the `docker login` that gives it a sign-in.

mod config;
mod helper;

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use bollard::auth::DockerCredentials;

use crate::module::running::container::docker::{DockerError, endpoint, without_secrets};

/// How the variables of this process's environment are read.
type Environment<'a> = &'a (dyn Fn(&str) -> Option<OsString> + Sync);

/// The sign-in to an image's registry that a pull is sent with, or why
/// there is none.
pub struct SignIn {
    registry: Registry,
    /// The account tay runs as, as a person names it.
    account: String,
    looked: Looked,
}

/// What the account's Docker configuration gave for the registry.
enum Looked {
    /// It was read, from `file`: the sign-in it holds, if it holds one.
    Read {
        file: PathBuf,
        sign_in: Option<DockerCredentials>,
    },
    /// It could not give one.
    Failed(Unfound),
}

/// Why the account's Docker configuration could not give a sign-in. Its
/// words follow "as", and never hold anything of a sign-in.
#[derive(Debug, thiserror::Error)]
enum Unfound {
    #[error("neither `DOCKER_CONFIG` nor `HOME` is set to say where it is")]
    Nowhere,
    #[error("`{}` cannot be read: {why}", file.display())]
    Unreadable { file: PathBuf, why: String },
    #[error("`{}` is not one tay can read, at line {line}, column {column}", file.display())]
    NotUnderstood {
        file: PathBuf,
        line: usize,
        column: usize,
    },
    #[error(
        "its sign-in for `{key}` in `{}` is not a name and a password in base64",
        file.display()
    )]
    Undecodable { file: PathBuf, key: String },
    #[error("the credential helper it names, `{program}`, is not installed")]
    HelperMissing { program: String },
    #[error("the credential helper it names, `{program}`, {why}")]
    HelperFailed { program: String, why: String },
}

impl SignIn {
    /// The sign-in for `image`'s registry that the account this process
    /// runs as has given Docker, or why there is none.
    pub async fn for_image(image: &str) -> SignIn {
        SignIn::looked_up(image, &|name| std::env::var_os(name)).await
    }

    async fn looked_up(image: &str, environment: Environment<'_>) -> SignIn {
        let registry = Registry::of(image);
        let account = match environment("USER").filter(|name| !name.is_empty()) {
            Some(name) => format!("`{}`", name.to_string_lossy()),
            None => format!("uid {}", rustix::process::getuid().as_raw()),
        };
        let looked = match endpoint::config_folder(environment) {
            None => Looked::Failed(Unfound::Nowhere),
            Some(folder) => {
                let file = folder.join("config.json");
                match config::sign_in(&file, registry.key(), environment).await {
                    Ok(sign_in) => Looked::Read { file, sign_in },
                    Err(unfound) => Looked::Failed(unfound),
                }
            }
        };
        SignIn {
            registry,
            account,
            looked,
        }
    }

    /// A copy of the sign-in, for the request to carry; `None` when there
    /// is none.
    pub fn credentials(&self) -> Option<DockerCredentials> {
        match &self.looked {
            Looked::Read { sign_in, .. } => sign_in.clone(),
            Looked::Failed(_) => None,
        }
    }

    /// `error`, which Docker gave a pull sent with this, with every secret
    /// of the sign-in taken out of its words, and with what went with the
    /// pull said where it may be why it was refused: whenever none could be
    /// had, and whenever the registry refused access.
    pub fn refused(&self, error: DockerError) -> DockerError {
        let error = without_secrets(error, &self.secrets());
        let DockerError::Refused { doing, why } = error else {
            return error;
        };
        let (registry, account) = (&self.registry, &self.account);
        let configuration =
            format!("the Docker configuration of the account tay runs as, {account}");
        let said = match &self.looked {
            Looked::Failed(unfound) => format!(
                "No sign-in to `{registry}` went with it: {configuration}, could not give one, as {unfound}"
            ),
            Looked::Read { .. } if !refuses_access(&why) => {
                return DockerError::Refused { doing, why };
            }
            Looked::Read {
                file,
                sign_in: None,
            } => format!(
                "No sign-in to `{registry}` went with it: {configuration}, has none for it in `{}`; `docker login {registry}` as {account} gives it one",
                file.display()
            ),
            Looked::Read {
                file,
                sign_in: Some(_),
            } => format!(
                "The sign-in to `{registry}` that went with it, from {configuration}, in `{}`, was refused; `docker login {registry}` as {account} gives it a new one",
                file.display()
            ),
        };
        DockerError::Refused {
            doing,
            why: format!("{why}. {said}"),
        }
    }

    /// The secrets of the sign-in, which nothing Docker says may show.
    fn secrets(&self) -> Vec<String> {
        let Looked::Read {
            sign_in: Some(sign_in),
            ..
        } = &self.looked
        else {
            return Vec::new();
        };
        [
            &sign_in.password,
            &sign_in.identitytoken,
            &sign_in.registrytoken,
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect()
    }
}

/// Whether Docker's words for a refused pull say that the registry refused
/// access to the image, in the words Docker and registries use when a
/// sign-in is wanted, or the one sent will not do. A registry that cannot
/// tell a stranger whether an image exists refuses access to one that does
/// not as well.
fn refuses_access(said: &str) -> bool {
    let said = said.to_lowercase();
    [
        "denied",
        "unauthorized",
        "forbidden",
        "authentication required",
        "no basic auth credentials",
        "insufficient_scope",
    ]
    .iter()
    .any(|words| said.contains(words))
}

/// The registry an image's name names, as Docker reads a name: its first
/// part, before a `/`, when that is `localhost` or holds a dot, a colon or
/// a capital letter, which a repository's name may not; otherwise Docker
/// Hub, which `docker.io` and `index.docker.io` also name.
#[derive(Debug, PartialEq)]
enum Registry {
    DockerHub,
    Named(String),
}

impl Registry {
    fn of(image: &str) -> Registry {
        let Some((first, _)) = image.split_once('/') else {
            return Registry::DockerHub;
        };
        let names_one = first == "localhost"
            || first.contains(['.', ':'])
            || first.chars().any(char::is_uppercase);
        match names_one && !matches!(first, "docker.io" | "index.docker.io") {
            true => Registry::Named(first.to_string()),
            false => Registry::DockerHub,
        }
    }

    /// What Docker's configuration keeps the registry's sign-in under:
    /// Docker Hub's under the address of its first interface, as `docker
    /// login` still writes it, and any other's under its name.
    fn key(&self) -> &str {
        match self {
            Registry::DockerHub => "https://index.docker.io/v1/",
            Registry::Named(name) => name,
        }
    }
}

/// The registry as `docker login` is given it.
impl fmt::Display for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Registry::DockerHub => "docker.io",
            Registry::Named(name) => name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_images_registry_is_its_first_part_where_docker_reads_one_there() {
        let named = |name: &str| Registry::Named(name.to_string());
        for (image, registry) in [
            ("busybox", Registry::DockerHub),
            ("busybox:1.36", Registry::DockerHub),
            ("library/busybox:1.36", Registry::DockerHub),
            ("example/server@sha256:0123", Registry::DockerHub),
            ("docker.io/library/busybox", Registry::DockerHub),
            ("index.docker.io/example/server", Registry::DockerHub),
            ("ghcr.io/example/server:1", named("ghcr.io")),
            ("localhost/server", named("localhost")),
            ("localhost:5000/server", named("localhost:5000")),
            ("registry:5000/server", named("registry:5000")),
            ("Registry/server", named("Registry")),
        ] {
            assert_eq!(Registry::of(image), registry, "{image}");
        }
        assert_eq!(Registry::DockerHub.key(), "https://index.docker.io/v1/");
        assert_eq!(Registry::DockerHub.to_string(), "docker.io");
        assert_eq!(named("ghcr.io").key(), "ghcr.io");
    }

    /// A sign-in to `ghcr.io` as `looked` says.
    fn looked(looked: Looked) -> SignIn {
        SignIn {
            registry: Registry::Named("ghcr.io".to_string()),
            account: "`builder`".to_string(),
            looked,
        }
    }

    fn read(sign_in: Option<DockerCredentials>) -> Looked {
        Looked::Read {
            file: PathBuf::from("/home/builder/.docker/config.json"),
            sign_in,
        }
    }

    fn refusal(why: &str) -> DockerError {
        DockerError::Refused {
            doing: "pull `ghcr.io/example/server:1`".to_string(),
            why: why.to_string(),
        }
    }

    const DENIED: &str = "failed to resolve reference: pull access denied, repository does not exist or may require authorization";

    #[test]
    fn a_refusal_of_access_with_no_sign_in_names_the_registry_the_account_and_docker_login() {
        let said = looked(read(None)).refused(refusal(DENIED)).to_string();
        assert_eq!(
            said,
            format!(
                "Docker could not pull `ghcr.io/example/server:1`: {DENIED}. No sign-in to `ghcr.io` went with it: the Docker configuration of the account tay runs as, `builder`, has none for it in `/home/builder/.docker/config.json`; `docker login ghcr.io` as `builder` gives it one"
            )
        );
    }

    #[test]
    fn a_refusal_of_the_sign_in_sent_says_so_and_never_shows_it() {
        let sign_in = DockerCredentials {
            username: Some("builder".to_string()),
            password: Some("hunter2".to_string()),
            ..DockerCredentials::default()
        };
        let said = looked(read(Some(sign_in)))
            .refused(refusal("401 Unauthorized for hunter2"))
            .to_string();
        assert_eq!(
            said,
            "Docker could not pull `ghcr.io/example/server:1`: 401 Unauthorized for <secret>. The sign-in to `ghcr.io` that went with it, from the Docker configuration of the account tay runs as, `builder`, in `/home/builder/.docker/config.json`, was refused; `docker login ghcr.io` as `builder` gives it a new one"
        );
    }

    #[test]
    fn a_configuration_that_could_not_give_a_sign_in_is_said_with_any_refusal() {
        let unfound = Unfound::HelperMissing {
            program: "docker-credential-gone".to_string(),
        };
        let said = looked(Looked::Failed(unfound))
            .refused(refusal("manifest unknown"))
            .to_string();
        assert_eq!(
            said,
            "Docker could not pull `ghcr.io/example/server:1`: manifest unknown. No sign-in to `ghcr.io` went with it: the Docker configuration of the account tay runs as, `builder`, could not give one, as the credential helper it names, `docker-credential-gone`, is not installed"
        );
    }

    #[test]
    fn a_refusal_that_is_not_of_access_and_docker_unreachable_say_nothing_of_a_sign_in() {
        let said = looked(read(None)).refused(refusal("manifest unknown"));
        assert_eq!(
            said.to_string(),
            "Docker could not pull `ghcr.io/example/server:1`: manifest unknown"
        );
        let unreachable = DockerError::Unreachable {
            socket: PathBuf::from("/var/run/docker.sock"),
            why: "connection refused".to_string(),
        };
        let said = looked(Looked::Failed(Unfound::Nowhere)).refused(unreachable);
        assert!(matches!(said, DockerError::Unreachable { .. }));
    }

    #[test]
    fn the_words_of_a_refused_access_are_told_from_others() {
        for refused in [
            DENIED,
            "pull access denied for example/server, repository does not exist or may require 'docker login'",
            "authorization failed: no basic auth credentials",
            "unexpected status from HEAD request to https://quay.io/v2/x/manifests/1: 401 UNAUTHORIZED",
            "error from registry: denied\ndenied",
            "error from registry: access forbidden",
            "unauthorized: authentication required",
            "insufficient_scope: authorization failed",
        ] {
            assert!(refuses_access(refused), "{refused}");
        }
        for other in [
            "docker.io/library/busybox:tay-test-no-such-tag: not found",
            "manifest unknown",
            "dial tcp 127.0.0.1:5000: connect: connection refused",
        ] {
            assert!(!refuses_access(other), "{other}");
        }
    }

    #[tokio::test]
    async fn the_configuration_is_the_one_docker_config_or_home_names_and_the_account_users() {
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join(".docker");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(
            folder.join("config.json"),
            r#"{"auths":{"ghcr.io":{"auth":"YnVpbGRlcjpodW50ZXIy"}}}"#,
        )
        .unwrap();
        let home_path = home.path().as_os_str().to_owned();
        let at_home = |name: &str| match name {
            "HOME" => Some(home_path.clone()),
            "USER" => Some(OsString::from("builder")),
            _ => None,
        };
        let sign_in = SignIn::looked_up("ghcr.io/example/server:1", &at_home).await;
        assert_eq!(sign_in.account, "`builder`");
        let credentials = sign_in.credentials().expect("a sign-in");
        assert_eq!(credentials.username.as_deref(), Some("builder"));
        assert_eq!(credentials.password.as_deref(), Some("hunter2"));
        // `DOCKER_CONFIG` wins over `HOME`, and names a folder that has no
        // configuration in it here.
        let elsewhere = tempfile::tempdir().unwrap();
        let elsewhere_path = elsewhere.path().as_os_str().to_owned();
        let configured = |name: &str| match name {
            "DOCKER_CONFIG" => Some(elsewhere_path.clone()),
            other => at_home(other),
        };
        let sign_in = SignIn::looked_up("ghcr.io/example/server:1", &configured).await;
        assert!(sign_in.credentials().is_none());
        assert!(matches!(
            &sign_in.looked,
            Looked::Read { file, sign_in: None } if *file == elsewhere.path().join("config.json")
        ));
        // With neither, there is no configuration to read, and an account
        // with no `USER` is named by its number.
        let sign_in = SignIn::looked_up("ghcr.io/example/server:1", &|_| None).await;
        assert!(matches!(sign_in.looked, Looked::Failed(Unfound::Nowhere)));
        assert!(sign_in.account.starts_with("uid "), "{}", sign_in.account);
    }
}
