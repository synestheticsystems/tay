//! Generating a secret once and keeping it, in a file of its own in the
//! state folder that only this account can read.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64ct::{Base64, Base64UrlUnpadded, Encoding};
use rand_core::{OsRng, RngCore};
use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::{Deserialize, Serialize};

use super::{PRIVATE, PUBLIC, SecretSettings, VALUE};
use crate::disk::{DiskError, write_secret};
use crate::module::operations::{self, Exists, Failure, Made, UnitPlace, Values, WrongType};
use crate::value::Filled;

/// How long a password is, in characters, and how many bytes `bytes`
/// makes, when `length` does not say.
const LENGTH: usize = 32;

/// The letters and digits a password is made of, so that it is safe in a
/// URL, a JSON string or a shell word.
const PASSWORD: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

#[derive(Debug, thiserror::Error)]
enum SecretError {
    #[error("`{setting}` is `{found}`, and is one of {wanted}")]
    NotOneOf {
        setting: &'static str,
        found: String,
        wanted: String,
    },
    #[error("it has not been generated")]
    Missing,
    #[error(
        "the one it keeps is a `{kept}`, and its settings now ask for a `{asked}`: `tay wipe {unit} --yes` makes a new one"
    )]
    OtherKind {
        kept: &'static str,
        asked: &'static str,
        unit: String,
    },
    #[error("the one it keeps, in `{}`, is not in a form this tay reads", .0.display())]
    Unread(std::path::PathBuf),
    #[error("the key pair cannot be made: {0}")]
    Keypair(String),
    #[error(transparent)]
    Disk(#[from] DiskError),
    #[error(transparent)]
    WrongType(#[from] WrongType),
}

/// What a secret's settings ask for, with every default applied.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Asked {
    Password { length: usize },
    Bytes { length: usize, encoding: Encoded },
    Keypair { algorithm: Algorithm },
}

impl Asked {
    /// The kind, as `kind` is written.
    fn kind(&self) -> &'static str {
        match self {
            Asked::Password { .. } => "password",
            Asked::Bytes { .. } => "bytes",
            Asked::Keypair { .. } => "keypair",
        }
    }

    /// The settings that differ in `now`, by name, when it asks for the
    /// same kind; `None` when it asks for another.
    fn differing(&self, now: &Asked) -> Option<Vec<&'static str>> {
        let named = |pairs: &[(&'static str, bool)]| {
            pairs
                .iter()
                .filter(|(_, differs)| *differs)
                .map(|(name, _)| *name)
                .collect()
        };
        match (self, now) {
            (Asked::Password { length }, Asked::Password { length: now }) => {
                Some(named(&[("length", length != now)]))
            }
            (
                Asked::Bytes { length, encoding },
                Asked::Bytes {
                    length: length_now,
                    encoding: encoding_now,
                },
            ) => Some(named(&[
                ("length", length != length_now),
                ("encoding", encoding != encoding_now),
            ])),
            (Asked::Keypair { algorithm }, Asked::Keypair { algorithm: now }) => {
                Some(named(&[("algorithm", algorithm != now)]))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Encoded {
    Hex,
    Base64,
    Base64url,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Algorithm {
    #[serde(rename = "rsa-2048")]
    Rsa2048,
}

/// A secret as it is stored: what it was generated for, and what it is.
#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(flatten)]
    asked: Asked,
    #[serde(flatten)]
    generated: Generated,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Generated {
    Value { value: String },
    Keypair { public: String, private: String },
}

impl Made for SecretSettings<Filled> {
    fn make(&self, place: &UnitPlace, _: bool) -> Result<Exists, Failure> {
        let asked = self.asked()?;
        let path = path(place);
        if let Some(stored) = read(&path)? {
            return Ok(kept(&stored, asked, place)?);
        }
        let stored = Stored {
            asked,
            generated: generate(asked)?,
        };
        keep(&path, &stored)?;
        Ok(exists(&stored, "generated"))
    }

    fn check(&self, place: &UnitPlace) -> Result<Exists, Failure> {
        let asked = self.asked()?;
        match read(&path(place))? {
            Some(stored) => Ok(kept(&stored, asked, place)?),
            None => Err(SecretError::Missing.into()),
        }
    }
}

/// The secret `stored` for the unit at `place`, which its settings now
/// ask to be as `asked`. Kept beats written: one generated with another
/// `length` or `encoding` is kept, saying so, since whatever already holds
/// it would not change with it; one of another kind cannot give what the
/// unit must give.
fn kept(stored: &Stored, asked: Asked, place: &UnitPlace) -> Result<Exists, SecretError> {
    let wipe = || format!("`tay wipe {} --yes` makes a new one", place.unit);
    let differing = stored
        .asked
        .differing(&asked)
        .ok_or_else(|| SecretError::OtherKind {
            kept: stored.asked.kind(),
            asked: asked.kind(),
            unit: place.unit.clone(),
        })?;
    let detail = match differing.as_slice() {
        [] => "kept".to_string(),
        settings => format!(
            "kept, though its settings now ask for another {}: {}",
            crate::diagnostic::quoted_list(settings),
            wipe()
        ),
    };
    Ok(exists(stored, &detail))
}

impl SecretSettings<Filled> {
    fn asked(&self) -> Result<Asked, SecretError> {
        let length = |length: &Option<Filled>| -> Result<usize, SecretError> {
            Ok(match length {
                Some(length) => usize::try_from(operations::number(length, "length")?)
                    .expect("a `length` the plan passes is from 1 to the longest there may be"),
                None => LENGTH,
            })
        };
        Ok(match self {
            SecretSettings::Password { length: written } => Asked::Password {
                length: length(written)?,
            },
            SecretSettings::Bytes {
                length: written,
                encoding,
            } => {
                let encoding = match encoding {
                    Some(encoding) => match operations::text(encoding, "encoding")? {
                        "hex" => Encoded::Hex,
                        "base64" => Encoded::Base64,
                        "base64url" => Encoded::Base64url,
                        other => return Err(not_one_of("encoding", other, super::ENCODINGS)),
                    },
                    None => Encoded::Hex,
                };
                Asked::Bytes {
                    length: length(written)?,
                    encoding,
                }
            }
            SecretSettings::Keypair { algorithm } => {
                match operations::text(algorithm, "algorithm")? {
                    "rsa-2048" => Asked::Keypair {
                        algorithm: Algorithm::Rsa2048,
                    },
                    other => return Err(not_one_of("algorithm", other, super::ALGORITHMS)),
                }
            }
        })
    }
}

fn not_one_of(setting: &'static str, found: &str, wanted: &[&str]) -> SecretError {
    SecretError::NotOneOf {
        setting,
        found: found.to_string(),
        wanted: crate::diagnostic::quoted_list(wanted),
    }
}

/// The file the unit's secret is kept in.
pub(super) fn path(place: &UnitPlace) -> PathBuf {
    place.environment.layout.generated_secret(&place.unit)
}

/// The secret stored at `path`; `None` when none is. One stored in a form
/// this tay does not read is kept as it is, rather than lost to a new one.
fn read(path: &Path) -> Result<Option<Stored>, SecretError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DiskError::io(path, "read")(error).into()),
    };
    // What the reader says of it is not given: it may quote the secret.
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|_| SecretError::Unread(path.to_path_buf()))
}

/// Keeps the secret where only this account can read it.
fn keep(path: &Path, stored: &Stored) -> Result<(), SecretError> {
    let json = serde_json::to_string(stored).expect("a stored secret serializes");
    Ok(write_secret(path, json.as_bytes())?)
}

fn generate(asked: Asked) -> Result<Generated, SecretError> {
    Ok(match asked {
        Asked::Password { length } => Generated::Value {
            value: password(length),
        },
        Asked::Bytes { length, encoding } => {
            let mut bytes = vec![0u8; length];
            OsRng.fill_bytes(&mut bytes);
            let value = match encoding {
                Encoded::Hex => bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
                Encoded::Base64 => Base64::encode_string(&bytes),
                Encoded::Base64url => Base64UrlUnpadded::encode_string(&bytes),
            };
            Generated::Value { value }
        }
        Asked::Keypair {
            algorithm: Algorithm::Rsa2048,
        } => {
            let failed = |error: &dyn std::fmt::Display| SecretError::Keypair(error.to_string());
            let private = RsaPrivateKey::new(&mut OsRng, 2048).map_err(|e| failed(&e))?;
            let public = RsaPublicKey::from(&private)
                .to_public_key_pem(LineEnding::LF)
                .map_err(|e| failed(&e))?;
            let private = private
                .to_pkcs8_pem(LineEnding::LF)
                .map_err(|e| failed(&e))?;
            Generated::Keypair {
                public,
                private: private.to_string(),
            }
        }
    })
}

/// `length` letters and digits, each as likely as any other.
fn password(length: usize) -> String {
    // A byte below the largest multiple of the alphabet's size picks a
    // character evenly; one above it is drawn again.
    let even = (256 / PASSWORD.len() * PASSWORD.len()) as u8;
    let mut password = String::with_capacity(length);
    let mut bytes = [0u8; 64];
    while password.len() < length {
        OsRng.fill_bytes(&mut bytes);
        for byte in bytes.iter().filter(|byte| **byte < even) {
            if password.len() == length {
                break;
            }
            password.push(char::from(PASSWORD[usize::from(*byte) % PASSWORD.len()]));
        }
    }
    password
}

fn exists(stored: &Stored, detail: &str) -> Exists {
    let values = match &stored.generated {
        Generated::Value { value } => Values::from([(VALUE.to_string(), Filled::secret(value))]),
        Generated::Keypair { public, private } => Values::from([
            (PUBLIC.to_string(), Filled::text(public)),
            (PRIVATE.to_string(), Filled::secret(private)),
        ]),
    };
    Exists {
        values,
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_is_letters_and_digits_of_its_length() {
        for length in [1, 32, 200] {
            let password = password(length);
            assert_eq!(password.len(), length);
            assert!(password.bytes().all(|byte| PASSWORD.contains(&byte)));
        }
        assert_ne!(password(32), password(32));
    }

    #[test]
    fn bytes_are_encoded_as_asked() {
        let value = |encoding| match generate(Asked::Bytes {
            length: 16,
            encoding,
        })
        .unwrap()
        {
            Generated::Value { value } => value,
            Generated::Keypair { .. } => unreachable!("bytes give one value"),
        };
        let hex = value(Encoded::Hex);
        assert_eq!(hex.len(), 32);
        assert!(hex.bytes().all(|byte| byte.is_ascii_hexdigit()));
        // 16 bytes are 24 characters of base64 with its padding, and 22
        // without.
        let padded = value(Encoded::Base64);
        assert!(padded.len() == 24 && padded.ends_with("=="));
        let url = value(Encoded::Base64url);
        assert!(url.len() == 22 && !url.contains(['=', '+', '/']));
    }

    #[test]
    fn a_kept_secret_differs_by_the_settings_it_was_generated_for_or_by_its_kind() {
        let bytes = |length, encoding| Asked::Bytes { length, encoding };
        let hex = bytes(16, Encoded::Hex);
        assert_eq!(hex.differing(&bytes(16, Encoded::Hex)), Some(vec![]));
        assert_eq!(
            hex.differing(&bytes(32, Encoded::Base64)),
            Some(vec!["length", "encoding"])
        );
        assert_eq!(hex.differing(&Asked::Password { length: 16 }), None);
    }

    #[test]
    fn a_stored_secret_says_what_it_was_generated_for() {
        let stored = Stored {
            asked: Asked::Bytes {
                length: 4,
                encoding: Encoded::Base64url,
            },
            generated: Generated::Value {
                value: "AAAAAA".to_string(),
            },
        };
        let json = serde_json::to_string(&stored).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"bytes","length":4,"encoding":"base64url","value":"AAAAAA"}"#
        );
        let read: Stored = serde_json::from_str(&json).unwrap();
        assert!(read.asked == stored.asked);
    }
}
