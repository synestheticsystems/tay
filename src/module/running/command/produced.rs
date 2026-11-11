//! The values a command produces: what its `check` or its `run` writes, as
//! one JSON object, to the file named by `TAY_VALUES`. Once the command is
//! done they are kept, so that a daemon started afresh gives them too: in
//! the unit's own folder, and a value declared secret with the secrets,
//! readable by this account only. An action's are not kept: they go to
//! whoever runs it. Nothing tay says of them quotes them.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value as Json};

use crate::disk::{DiskError, remove_if_there, write_secret, write_whole};
use crate::module::operations::{Failure, UnitPlace, Values};
use crate::syntax::names::NameAt;
use crate::value::{Exact, Filled};

/// The file `TAY_VALUES` names, in the unit's own folder.
const WRITTEN: &str = "produced.json";

/// The values kept that are not secret, in the unit's own folder.
const KEPT: &str = "values.json";

#[derive(Debug, thiserror::Error)]
enum ProducedError {
    #[error("`{}`, the file `TAY_VALUES` names, does not hold one JSON object", .0.display())]
    NotAnObject(PathBuf),
    #[error("it gave no value for `{0}`, which it lists under `produces`")]
    Missing(String),
    #[error("it gave `{0}`, which it does not list under `produces`")]
    Undeclared(String),
    #[error(
        "it gave `{name}` as {kind}, and a produced value is text, a whole number or true or false"
    )]
    Kind { name: String, kind: &'static str },
    #[error("`{}`, where its values were kept, is not in a form this tay reads", .0.display())]
    Unread(PathBuf),
    #[error(
        "no value for `{name}` is kept from when it last ran: `tay wipe {unit} --yes` runs it again"
    )]
    NotKept { name: String, unit: String },
    #[error(transparent)]
    Disk(#[from] DiskError),
}

/// The file the command writes its values to, which `TAY_VALUES` names.
pub fn file(place: &UnitPlace) -> PathBuf {
    place
        .environment
        .layout
        .unit_state(&place.unit)
        .join(WRITTEN)
}

/// Removes what an earlier step wrote, so that only what the step about to
/// run writes is read.
pub fn clear(place: &UnitPlace) -> Result<(), Failure> {
    let path = file(place);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ProducedError::Disk(DiskError::io(path, "removed")(error)).into()),
    }
}

/// Whether the step that ran last wrote values that are not kept yet.
pub fn written(place: &UnitPlace) -> bool {
    file(place).exists()
}

/// The values the step that ran wrote, checked against the names `declared`
/// with whether each is secret, and kept when the command `keeps` its
/// values.
pub fn take(
    place: &UnitPlace,
    declared: &[(NameAt, bool)],
    keeps: bool,
) -> Result<Values, Failure> {
    let path = file(place);
    let written = match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Json>(&text) {
            // Neither the text nor what the parser says of it is given: it
            // may quote a secret.
            Ok(Json::Object(object)) => object,
            _ => return Err(ProducedError::NotAnObject(path).into()),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(error) => return Err(ProducedError::Disk(DiskError::io(path, "read")(error)).into()),
    };
    if let Some(name) = written
        .keys()
        .find(|name| !declared.iter().any(|(declared, _)| declared.name == **name))
    {
        return Err(ProducedError::Undeclared(name.clone()).into());
    }
    let mut values = Values::new();
    let mut plain = Map::new();
    let mut secret = Map::new();
    for (name, is_secret) in declared {
        let value = written
            .get(&name.name)
            .ok_or_else(|| ProducedError::Missing(name.name.clone()))?;
        values.insert(
            name.name.clone(),
            Filled::of(exact(&name.name, value)?, *is_secret),
        );
        match is_secret {
            true => secret.insert(name.name.clone(), value.clone()),
            false => plain.insert(name.name.clone(), value.clone()),
        };
    }
    if keeps {
        keep(place, &plain, &secret)?;
    }
    clear(place)?;
    Ok(values)
}

/// Forgets the values kept when the command was last done.
pub fn forget(place: &UnitPlace) -> Result<(), Failure> {
    remove_if_there(&kept_path(place))?;
    Ok(remove_if_there(&secrets(place))?)
}

/// The values kept when the command was last done. A value it declares
/// that is not among them fails it, rather than be given as nothing, as
/// when it declares one it did not produce then, or what was kept is gone.
pub fn kept(place: &UnitPlace, declared: &[(NameAt, bool)]) -> Result<Values, Failure> {
    let plain = read_kept(&kept_path(place))?;
    let secret = read_kept(&secrets(place))?;
    let mut values = Values::new();
    for (name, is_secret) in declared {
        let kept = match is_secret {
            true => secret.get(&name.name),
            false => plain.get(&name.name),
        };
        let kept = kept.ok_or_else(|| ProducedError::NotKept {
            name: name.name.clone(),
            unit: place.unit.clone(),
        })?;
        values.insert(
            name.name.clone(),
            Filled::of(exact(&name.name, kept)?, *is_secret),
        );
    }
    Ok(values)
}

/// A produced value as tay holds one.
fn exact(name: &str, value: &Json) -> Result<Exact, ProducedError> {
    let kind = |kind| ProducedError::Kind {
        name: name.to_string(),
        kind,
    };
    match value {
        Json::String(text) => Ok(Exact::Text(text.clone())),
        Json::Bool(b) => Ok(Exact::Bool(*b)),
        Json::Number(number) => number
            .as_i64()
            .map(Exact::Number)
            .ok_or_else(|| kind("a number that is not whole")),
        Json::Null => Err(kind("null")),
        Json::Array(_) => Err(kind("a list")),
        Json::Object(_) => Err(kind("a mapping")),
    }
}

fn kept_path(place: &UnitPlace) -> PathBuf {
    place.environment.layout.unit_state(&place.unit).join(KEPT)
}

fn secrets(place: &UnitPlace) -> PathBuf {
    place.environment.layout.produced_secrets(&place.unit)
}

fn keep(
    place: &UnitPlace,
    plain: &Map<String, Json>,
    secret: &Map<String, Json>,
) -> Result<(), ProducedError> {
    let kept = kept_path(place);
    // A command that runs in a container has nothing of its own on the
    // host until its values are kept.
    if let Some(folder) = kept.parent() {
        fs::create_dir_all(folder).map_err(DiskError::io(folder, "made"))?;
    }
    write_whole(&kept, json(plain).as_bytes(), 0o644)?;
    let path = secrets(place);
    if secret.is_empty() {
        return match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DiskError::io(path, "removed")(error).into()),
        };
    }
    Ok(write_secret(&path, json(secret).as_bytes())?)
}

fn json(values: &Map<String, Json>) -> String {
    let text = serde_json::to_string_pretty(values).expect("values serialize");
    format!("{text}\n")
}

/// The values kept at `path`; none when nothing is.
fn read_kept(path: &Path) -> Result<Map<String, Json>, ProducedError> {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(Json::Object(object)) => Ok(object),
            _ => Err(ProducedError::Unread(path.to_path_buf())),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
        Err(error) => Err(DiskError::io(path, "read")(error).into()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use super::*;
    use crate::diagnostic::source::{Loc, Sources};
    use crate::disk::{Environment, Layout, Machine};

    /// The command `issuer` of an environment in `root`, which nothing has
    /// brought up: only its folder is used.
    fn place(root: &Path) -> UnitPlace {
        UnitPlace {
            unit: "issuer".to_string(),
            environment: Arc::new(Environment {
                layout: Layout::of(root),
                id: serde_json::from_str("\"0123456789ab\"").unwrap(),
                machine: Machine::from_env().unwrap(),
            }),
        }
    }

    /// The names a command declares under `produces`, each with whether it
    /// is secret.
    fn produces(names: &[(&str, bool)]) -> Vec<(NameAt, bool)> {
        let source = Sources::default().add_argument("produces");
        (names.iter())
            .map(|(name, secret)| {
                let name = NameAt {
                    name: name.to_string(),
                    loc: Loc::new(source, 1, 1),
                };
                (name, *secret)
            })
            .collect()
    }

    /// The command at `place` writing `text` to the file `TAY_VALUES` names.
    fn writes(place: &UnitPlace, text: &str) {
        let file = file(place);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn values_are_given_and_kept_plain_in_the_units_folder_and_secret_ones_for_this_account_alone()
    {
        let root = tempfile::tempdir().unwrap();
        let place = place(root.path());
        let layout = &place.environment.layout;
        // A `secret` unit of the same name, as a unit with two ways has,
        // keeps its value apart.
        let generated = layout.generated_secret("issuer");
        write_secret(&generated, b"generated").unwrap();
        let declared = produces(&[("id", false), ("token", true), ("fresh", false)]);
        writes(&place, r#"{"id": 42, "token": "tok-1", "fresh": true}"#);
        let given = Values::from([
            ("id".to_string(), Filled::number(42)),
            ("token".to_string(), Filled::secret("tok-1")),
            ("fresh".to_string(), Filled::of(Exact::Bool(true), false)),
        ]);
        assert_eq!(take(&place, &declared, true).unwrap(), given);
        assert!(!written(&place), "what it wrote is read once");
        let plain = fs::read_to_string(kept_path(&place)).unwrap();
        assert!(
            plain.contains("\"id\": 42") && !plain.contains("tok-1"),
            "{plain}"
        );
        let secret = layout.produced_secrets("issuer");
        assert!(fs::read_to_string(&secret).unwrap().contains("tok-1"));
        assert_eq!(
            (mode(&secret), mode(secret.parent().unwrap())),
            (0o600, 0o700)
        );
        // A daemon started afresh is given what was kept.
        assert_eq!(kept(&place, &declared).unwrap(), given);
        // A run that produces no secret keeps none.
        let plain_only = produces(&[("id", false)]);
        writes(&place, r#"{"id": 43}"#);
        take(&place, &plain_only, true).unwrap();
        assert!(!secret.exists());
        assert_eq!(
            kept(&place, &plain_only).unwrap(),
            Values::from([("id".to_string(), Filled::number(43))])
        );
        forget(&place).unwrap();
        assert!(!kept_path(&place).exists());
        assert_eq!(fs::read(&generated).unwrap(), b"generated");
    }

    #[test]
    fn an_actions_values_are_given_and_not_kept() {
        let root = tempfile::tempdir().unwrap();
        let place = place(root.path());
        writes(&place, r#"{"token": "tok"}"#);
        let declared = produces(&[("token", true)]);
        assert_eq!(
            take(&place, &declared, false).unwrap(),
            Values::from([("token".to_string(), Filled::secret("tok"))])
        );
        assert!(!kept_path(&place).exists());
        assert!(!place.environment.layout.produced_secrets("issuer").exists());
    }

    #[test]
    fn a_value_it_declares_must_be_written_and_one_it_does_not_must_not() {
        let root = tempfile::tempdir().unwrap();
        let place = place(root.path());
        let declared = produces(&[("id", false)]);
        writes(&place, "{}");
        assert_eq!(
            take(&place, &declared, true).unwrap_err().to_string(),
            "it gave no value for `id`, which it lists under `produces`"
        );
        writes(&place, r#"{"id": 1, "extra": "x"}"#);
        assert_eq!(
            take(&place, &declared, true).unwrap_err().to_string(),
            "it gave `extra`, which it does not list under `produces`"
        );
        assert!(!kept_path(&place).exists(), "nothing is kept of a refusal");
    }

    #[test]
    fn a_value_is_text_a_whole_number_or_true_or_false() {
        let root = tempfile::tempdir().unwrap();
        let place = place(root.path());
        let declared = produces(&[("id", true)]);
        for (written, kind) in [
            ("[1, 2]", "a list"),
            (r#"{"a": 1}"#, "a mapping"),
            ("null", "null"),
            ("1.5", "a number that is not whole"),
        ] {
            writes(&place, &format!(r#"{{"id": {written}}}"#));
            assert_eq!(
                take(&place, &declared, true).unwrap_err().to_string(),
                format!(
                    "it gave `id` as {kind}, and a produced value is text, a whole number or true or false"
                )
            );
        }
    }

    #[test]
    fn a_file_that_is_not_one_json_object_is_refused_without_being_quoted() {
        let root = tempfile::tempdir().unwrap();
        let place = place(root.path());
        let declared = produces(&[("token", true)]);
        for written in [r#"["hunter2"]"#, r#""hunter2""#, "token=hunter2", ""] {
            writes(&place, written);
            let refused = take(&place, &declared, true).unwrap_err().to_string();
            assert_eq!(
                refused,
                format!(
                    "`{}`, the file `TAY_VALUES` names, does not hold one JSON object",
                    file(&place).display()
                )
            );
        }
    }

    #[test]
    fn kept_values_that_cannot_be_read_fail_it_by_where_they_are_kept() {
        let root = tempfile::tempdir().unwrap();
        let place = place(root.path());
        let declared = produces(&[("id", false), ("token", true)]);
        writes(&place, r#"{"id": 1, "token": "tok"}"#);
        take(&place, &declared, true).unwrap();
        let secret = place.environment.layout.produced_secrets("issuer");
        for path in [kept_path(&place), secret] {
            let was = fs::read(&path).unwrap();
            fs::write(&path, "hunter2").unwrap();
            assert_eq!(
                kept(&place, &declared).unwrap_err().to_string(),
                format!(
                    "`{}`, where its values were kept, is not in a form this tay reads",
                    path.display()
                )
            );
            fs::write(&path, was).unwrap();
        }
    }

    #[test]
    fn a_value_not_kept_from_when_it_last_ran_is_not_given_and_wiping_runs_it_again() {
        let root = tempfile::tempdir().unwrap();
        let place = place(root.path());
        let declared = produces(&[("id", false), ("token", true)]);
        writes(&place, r#"{"id": 1, "token": "tok"}"#);
        take(&place, &declared, true).unwrap();
        // As in an environment whose secret values were kept where an older
        // tay kept them.
        fs::remove_file(place.environment.layout.produced_secrets("issuer")).unwrap();
        assert_eq!(
            kept(&place, &declared).unwrap_err().to_string(),
            "no value for `token` is kept from when it last ran: `tay wipe issuer --yes` runs it again"
        );
    }
}
