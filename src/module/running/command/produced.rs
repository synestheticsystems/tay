//! The values a command produces: what its `check` or its `run` writes, as
//! one JSON object, to the file named by `TAY_VALUES`. Once the command is
//! done they are kept, so that a daemon started afresh gives them too: in
//! the unit's own folder, and a value declared secret with the secrets,
//! readable by this account only. An action's are not kept: they go to
//! whoever runs it. Nothing tay says of them quotes them.

use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value as Json};

use crate::disk::{DiskError, remove_if_there, write_whole};
use crate::module::operations::{Failure, UnitPlace, Values};
use crate::syntax::names::NameAt;
use crate::value::{Exact, Filled};

/// The file `TAY_VALUES` names, in the unit's own folder.
const WRITTEN: &str = "produced.json";

/// The values kept that are not secret, in the unit's own folder.
const KEPT: &str = "values.json";

/// The folder in the state folder that secrets are kept in, one file for
/// each unit that has any.
const SECRETS: &str = "secrets";

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
    let mut plain = Map::new();
    let mut secret = Map::new();
    for (name, is_secret) in declared {
        let value = written
            .get(&name.name)
            .ok_or_else(|| ProducedError::Missing(name.name.clone()))?;
        exact(&name.name, value)?;
        match is_secret {
            true => secret.insert(name.name.clone(), value.clone()),
            false => plain.insert(name.name.clone(), value.clone()),
        };
    }
    if keeps {
        keep(place, &plain, &secret)?;
    }
    clear(place)?;
    values(declared, &plain, &secret)
}

/// Forgets the values kept when the command was last done.
pub fn forget(place: &UnitPlace) -> Result<(), Failure> {
    remove_if_there(&kept_path(place))?;
    Ok(remove_if_there(&secrets(place))?)
}

/// The values kept when the command was last done.
pub fn kept(place: &UnitPlace, declared: &[(NameAt, bool)]) -> Result<Values, Failure> {
    let plain = read_kept(&kept_path(place))?;
    let secret = read_kept(&secrets(place))?;
    values(declared, &plain, &secret)
}

fn values(
    declared: &[(NameAt, bool)],
    plain: &Map<String, Json>,
    secret: &Map<String, Json>,
) -> Result<Values, Failure> {
    let mut values = BTreeMap::new();
    for (name, is_secret) in declared {
        let kept = match is_secret {
            true => secret.get(&name.name),
            false => plain.get(&name.name),
        };
        let kept = kept.ok_or_else(|| ProducedError::Missing(name.name.clone()))?;
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
    place
        .environment
        .layout
        .state()
        .join(SECRETS)
        .join(&place.unit)
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
    let folder = path
        .parent()
        .expect("a secret's file is in the secrets folder");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
        .and_then(|()| fs::set_permissions(folder, Permissions::from_mode(0o700)))
        .map_err(DiskError::io(folder, "made"))?;
    Ok(write_whole(&path, json(secret).as_bytes(), 0o600)?)
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
