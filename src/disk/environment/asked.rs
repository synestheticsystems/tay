//! What an environment was last asked for: the profile, each `--set` and the
//! units named on `tay up`. The environment remembers them, so that a later
//! plain `tay up`, a `tay status`, a `tay plan` and a daemon started afresh
//! all see the same stack. The file is readable by this account only,
//! because a `--set` may carry a secret setting's value.

use std::io;

use serde::{Deserialize, Serialize};

use super::Layout;
use crate::disk::{DiskError, write_whole};

/// The choices a command makes, as the environment remembers them. It has
/// no `Debug`, since a `--set` may give a secret setting its value.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asked {
    pub profile: Option<String>,
    /// Each `--set PLACE=VALUE`, in the order they apply.
    pub sets: Vec<String>,
    /// The units named on `tay up`; none brings up what the choices list
    /// under `up`.
    pub units: Vec<String>,
}

impl Asked {
    /// What the environment remembers; nothing when it has never been asked.
    pub fn read(layout: &Layout) -> Result<Asked, DiskError> {
        let path = layout.asked();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).map_err(|source| DiskError::Unreadable { path, source })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Asked::default()),
            Err(error) => Err(DiskError::io(path, "read")(error)),
        }
    }

    pub fn write(&self, layout: &Layout) -> Result<(), DiskError> {
        let state = layout.state();
        std::fs::create_dir_all(&state).map_err(DiskError::io(&state, "made"))?;
        let json = serde_json::to_string_pretty(self).expect("what was asked serializes");
        write_whole(&layout.asked(), format!("{json}\n").as_bytes(), 0o600)
    }

    /// What is asked when a command adds `profile`, `sets` and `units` to
    /// what is remembered: `profile` replaces the remembered profile, each
    /// of `sets` replaces a remembered one for the same place and comes
    /// after the rest, and units named replace the units remembered.
    pub fn with(mut self, profile: Option<String>, sets: &[String], units: &[String]) -> Asked {
        if profile.is_some() {
            self.profile = profile;
        }
        self.sets.retain(|kept| {
            !sets
                .iter()
                .any(|set| Asked::place(set) == Asked::place(kept))
        });
        self.sets.extend(sets.iter().cloned());
        if !units.is_empty() {
            self.units = units.to_vec();
        }
        self
    }

    /// What is asked once the `--set`s for `places` are forgotten, so that
    /// what the profile or the stack gives there holds again.
    pub fn without(mut self, places: &[String]) -> Asked {
        self.sets.retain(|kept| {
            !places
                .iter()
                .any(|forgotten| Asked::place(kept) == forgotten)
        });
        self
    }

    /// Whether a `--set` for `place` is remembered.
    pub fn remembers(&self, wanted: &str) -> bool {
        self.sets.iter().any(|kept| Asked::place(kept) == wanted)
    }

    pub fn is_empty(&self) -> bool {
        *self == Asked::default()
    }

    /// The place a `--set PLACE=VALUE` sets.
    pub fn place(set: &str) -> &str {
        set.split_once('=').map_or(set, |(place, _)| place)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn a_new_choice_replaces_the_remembered_one_for_its_place_and_the_rest_are_kept() {
        let remembered = Asked {
            profile: Some("dev".to_string()),
            sets: strings(&["settings.a=1", "up_add=tools", "settings.b=2"]),
            units: strings(&["web"]),
        };
        let asked = remembered
            .clone()
            .with(None, &strings(&["settings.a=3"]), &[]);
        assert_eq!(asked.profile.as_deref(), Some("dev"));
        assert_eq!(
            asked.sets,
            strings(&["up_add=tools", "settings.b=2", "settings.a=3"])
        );
        assert_eq!(asked.units, strings(&["web"]));
        let asked = remembered
            .clone()
            .with(Some("ci".to_string()), &[], &strings(&["api"]));
        assert_eq!(
            (asked.profile.as_deref(), asked.units),
            (Some("ci"), strings(&["api"]))
        );
        // Forgetting a place forgets its `--set` alone.
        assert!(remembered.remembers("up_add"));
        let asked = remembered.without(&strings(&["up_add", "settings.c"]));
        assert_eq!(asked.sets, strings(&["settings.a=1", "settings.b=2"]));
        assert!(!asked.remembers("up_add"));
        assert_eq!(asked.profile.as_deref(), Some("dev"));
    }

    #[test]
    fn what_was_asked_is_kept_for_this_account_alone() {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::of(root.path());
        assert!(Asked::read(&layout).unwrap().is_empty());
        let asked = Asked::default().with(None, &strings(&["settings.key=hunter2"]), &[]);
        asked.write(&layout).unwrap();
        assert!(Asked::read(&layout).unwrap() == asked);
        let mode = std::fs::metadata(layout.asked())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
