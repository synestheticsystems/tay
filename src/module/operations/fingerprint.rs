//! What a unit that runs was started from: each of its settings by the
//! name its module gives it, and each file it uses that tay knows of by the
//! name the plan gives it, each as a digest. Only digests are kept, so no
//! value is, a secret least of all; and each is salted with the
//! environment's id, so that the fingerprints of two environments cannot
//! be compared to find a secret they share.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::UnitPlace;
use crate::disk::DiskError;
use crate::value::Filled;

/// What a file that is not there is kept as, in place of a digest of its
/// content.
const MISSING: &str = "missing";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    settings: BTreeMap<String, String>,
    files: BTreeMap<String, String>,
}

/// What differs between two fingerprints: the settings and the files, by
/// name, each in order. One that is in only one of them differs.
#[derive(Debug, PartialEq, Eq)]
pub struct Changed {
    pub settings: Vec<String>,
    pub files: Vec<String>,
}

impl Fingerprint {
    /// What differs in `now` from what this fingerprint says.
    pub fn changed(&self, now: &Fingerprint) -> Changed {
        Changed {
            settings: differing(&self.settings, &now.settings),
            files: differing(&self.files, &now.files),
        }
    }
}

fn differing(was: &BTreeMap<String, String>, now: &BTreeMap<String, String>) -> Vec<String> {
    let mut names: Vec<&String> = was.keys().chain(now.keys()).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|name| was.get(*name) != now.get(*name))
        .cloned()
        .collect()
}

impl Changed {
    pub fn is_empty(&self) -> bool {
        self.settings.is_empty() && self.files.is_empty()
    }
}

/// What changed as a person reads it: "`command`, `env.MODE` and the file
/// `conf`".
impl fmt::Display for Changed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let settings = self.settings.iter().map(|name| format!("`{name}`"));
        let files = self.files.iter().map(|name| format!("the file `{name}`"));
        let all: Vec<String> = settings.chain(files).collect();
        match all.as_slice() {
            [] => Ok(()),
            [one] => f.write_str(one),
            [rest @ .., last] => write!(f, "{} and {last}", rest.join(", ")),
        }
    }
}

/// Takes the fingerprint of what a unit of one environment is started
/// from.
pub struct Taking {
    salt: String,
    taken: Fingerprint,
}

impl Taking {
    /// Begins the fingerprint of the unit at `place`.
    pub fn of(place: &UnitPlace) -> Taking {
        Taking::adding_to(place, Fingerprint::default())
    }

    /// Goes on with `taken`, the fingerprint of the unit at `place` as far
    /// as it was taken.
    pub fn adding_to(place: &UnitPlace, taken: Fingerprint) -> Taking {
        Taking::salted_with(place.environment.id.as_str(), taken)
    }

    fn salted_with(salt: &str, taken: Fingerprint) -> Taking {
        Taking {
            salt: salt.to_string(),
            taken,
        }
    }

    /// Takes the setting `name`, whose value is `value` in any form that
    /// serializes, each secret in it revealed: only its digest is kept.
    pub fn setting(&mut self, name: impl Into<String>, value: &impl Serialize) {
        let text = serde_json::to_string(value).expect("a setting's value serializes");
        let digest = self.salted().chain_update(text.as_bytes()).finalize();
        self.taken.settings.insert(name.into(), hex(&digest));
    }

    /// Takes the setting `name`, one filled value.
    pub fn value(&mut self, name: impl Into<String>, value: &Filled) {
        self.setting(name, &value.reveal());
    }

    /// Takes the setting `name`, a list of filled values.
    pub fn values(&mut self, name: impl Into<String>, values: &[Filled]) {
        let revealed: Vec<String> = values.iter().map(Filled::reveal).collect();
        self.setting(name, &revealed);
    }

    /// Takes the file `name`, at `path`, by its content, or as missing
    /// when it is not there.
    pub fn file(&mut self, name: impl Into<String>, path: &Path) -> Result<(), DiskError> {
        let unreadable = DiskError::io(path, "read");
        let digest = match std::fs::File::open(path) {
            Ok(mut file) => {
                let mut hasher = self.salted();
                let mut buffer = vec![0; 64 * 1024];
                loop {
                    match file.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => hasher.update(&buffer[..read]),
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => return Err(unreadable(error)),
                    }
                }
                hex(&hasher.finalize())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => MISSING.to_string(),
            Err(error) => return Err(unreadable(error)),
        };
        self.taken.files.insert(name.into(), digest);
        Ok(())
    }

    pub fn taken(self) -> Fingerprint {
        self.taken
    }

    /// A digest begun with the environment's salt, ended by a byte no
    /// salt holds.
    fn salted(&self) -> Sha256 {
        Sha256::new()
            .chain_update(self.salt.as_bytes())
            .chain_update([0])
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fingerprint, salted with `salt`, of a unit started with the
    /// secret `token` and the file at `path`.
    fn taken(salt: &str, token: &str, path: &Path) -> Fingerprint {
        let mut taking = Taking::salted_with(salt, Fingerprint::default());
        taking.setting("command", &["serve", "--fast"]);
        taking.setting("secret_env.TOKEN", &token);
        taking.file("conf", path).unwrap();
        taking.taken()
    }

    #[test]
    fn what_changed_is_named_and_no_value_is_kept() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("conf");
        std::fs::write(&path, "one").unwrap();
        let was = taken("env-a", "hunter2", &path);
        let kept = serde_json::to_string(&was).unwrap();
        assert!(!kept.contains("hunter2") && !kept.contains("one"), "{kept}");
        assert!(was.changed(&taken("env-a", "hunter2", &path)).is_empty());
        std::fs::write(&path, "two").unwrap();
        let now = taken("env-a", "other", &path);
        let changed = was.changed(&now);
        assert_eq!(changed.settings, ["secret_env.TOKEN"]);
        assert_eq!(changed.files, ["conf"]);
        assert_eq!(
            changed.to_string(),
            "`secret_env.TOKEN` and the file `conf`"
        );
        std::fs::remove_file(&path).unwrap();
        let gone = taken("env-a", "other", &path);
        assert_eq!(now.changed(&gone).to_string(), "the file `conf`");
    }

    #[test]
    fn a_setting_written_or_left_out_since_has_changed() {
        let mut taking = Taking::salted_with("env-a", Fingerprint::default());
        taking.setting("env.MODE", &"fast");
        let one = taking.taken();
        let mut taking = Taking::salted_with("env-a", Fingerprint::default());
        taking.setting("workdir", &"/srv");
        let other = taking.taken();
        assert_eq!(one.changed(&other).to_string(), "`env.MODE` and `workdir`");
    }

    #[test]
    fn one_secret_in_two_environments_has_two_digests() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("conf");
        let one = taken("env-a", "hunter2", &path);
        let other = taken("env-b", "hunter2", &path);
        assert_eq!(
            one.changed(&other).settings,
            ["command", "secret_env.TOKEN"]
        );
    }
}
