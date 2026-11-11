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

/// What a command plans with: what it adds, and what the environment
/// remembers being asked for that it leaves in force. They are kept apart
/// so that a remembered choice that no longer fits the stack, as when a
/// change of branch takes away a setting or a unit it names, is said to be
/// remembered, with how to forget it. Layered in that order, what is
/// remembered first, they are what is asked. It has no `Debug`, as
/// [`Asked`] has none.
#[derive(Clone, Default)]
pub struct Asking {
    /// What is remembered but for what `adds` replaces.
    pub remembered: Asked,
    /// The profile, the `--set`s and the units the command names.
    pub adds: Asked,
}

impl Asking {
    /// The two as one, as the environment remembers them once the command
    /// is taken: what is left of what was remembered, then what the command
    /// adds.
    pub fn joined(self) -> Asked {
        let Asking { remembered, adds } = self;
        Asked {
            profile: adds.profile.or(remembered.profile),
            sets: [remembered.sets, adds.sets].concat(),
            units: [remembered.units, adds.units].concat(),
        }
    }
}

impl Asked {
    /// The place `tay set` and `tay unset` take for the profile, which
    /// `--set` does not.
    pub const PROFILE: &str = "profile";
    /// The place `tay unset` takes for the units named on an earlier
    /// `tay up`, which neither `tay set` nor `--set` takes.
    pub const UNITS: &str = "units";

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
        layout.make_state()?;
        let json = serde_json::to_string_pretty(self).expect("what was asked serializes");
        write_whole(&layout.asked(), format!("{json}\n").as_bytes(), 0o600)
    }

    /// What a command plans with when it `adds` choices to `self`, what is
    /// remembered: the profile it names replaces the remembered profile,
    /// each of its `--set`s replaces a remembered one for the same place
    /// and comes after the rest, and units it names replace the units
    /// remembered.
    pub fn asking(mut self, adds: Asked) -> Asking {
        if adds.profile.is_some() {
            self.profile = None;
        }
        self.sets.retain(|kept| {
            !adds
                .sets
                .iter()
                .any(|set| Asked::place(set) == Asked::place(kept))
        });
        if !adds.units.is_empty() {
            self.units.clear();
        }
        Asking {
            remembered: self,
            adds,
        }
    }

    /// What is asked when a command adds `profile`, `sets` and `units` to
    /// what is remembered, as [`Asked::asking`] says.
    pub fn with(self, profile: Option<String>, sets: &[String], units: &[String]) -> Asked {
        let adds = Asked {
            profile,
            sets: sets.to_vec(),
            units: units.to_vec(),
        };
        self.asking(adds).joined()
    }

    /// What is asked once what is remembered for `places` is forgotten: the
    /// `--set` for each, so that what the profile or the stack gives there
    /// holds again; the profile for [`Asked::PROFILE`], so that the
    /// environment's file or the stack's default chooses it again; and the
    /// units for [`Asked::UNITS`], so that what the choices list under `up`
    /// comes up again.
    pub fn without(mut self, places: &[String]) -> Asked {
        let forgets = |place: &str| places.iter().any(|forgotten| forgotten == place);
        if forgets(Asked::PROFILE) {
            self.profile = None;
        }
        if forgets(Asked::UNITS) {
            self.units.clear();
        }
        self.sets.retain(|kept| !forgets(Asked::place(kept)));
        self
    }

    /// Whether anything is remembered for `place`: a profile for
    /// [`Asked::PROFILE`], units for [`Asked::UNITS`], or a `--set` for any
    /// other.
    pub fn remembers(&self, place: &str) -> bool {
        match place {
            Asked::PROFILE => self.profile.is_some(),
            Asked::UNITS => !self.units.is_empty(),
            place => self.sets.iter().any(|kept| Asked::place(kept) == place),
        }
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
        // The profile is a place of its own, forgotten alone.
        assert!(asked.remembers(Asked::PROFILE));
        let asked = asked.without(&strings(&[Asked::PROFILE]));
        assert_eq!(asked.profile, None);
        assert!(!asked.remembers(Asked::PROFILE));
        assert_eq!(asked.sets, strings(&["settings.a=1", "settings.b=2"]));
        // So are the units named on an earlier `tay up`.
        assert!(asked.remembers(Asked::UNITS));
        let asked = asked.without(&strings(&[Asked::UNITS]));
        assert!(asked.units.is_empty());
        assert!(!asked.remembers(Asked::UNITS));
        assert_eq!(asked.sets, strings(&["settings.a=1", "settings.b=2"]));
    }

    #[test]
    fn what_a_command_adds_is_kept_apart_from_what_is_remembered_that_it_leaves_in_force() {
        let remembered = Asked {
            profile: Some("dev".to_string()),
            sets: strings(&["settings.a=1", "settings.b=2"]),
            units: strings(&["web"]),
        };
        let adds = Asked {
            profile: None,
            sets: strings(&["settings.a=3"]),
            units: Vec::new(),
        };
        let asking = remembered.clone().asking(adds);
        assert_eq!(asking.remembered.profile.as_deref(), Some("dev"));
        assert_eq!(asking.remembered.sets, strings(&["settings.b=2"]));
        assert_eq!(asking.remembered.units, strings(&["web"]));
        assert_eq!(asking.adds.sets, strings(&["settings.a=3"]));
        // What replaces the remembered profile or units leaves none of
        // them remembered.
        let adds = Asked {
            profile: Some("ci".to_string()),
            sets: Vec::new(),
            units: strings(&["api"]),
        };
        let asking = remembered.asking(adds);
        assert_eq!(asking.remembered.profile, None);
        assert!(asking.remembered.units.is_empty());
        let joined = asking.joined();
        assert_eq!(
            (joined.profile.as_deref(), joined.units),
            (Some("ci"), strings(&["api"]))
        );
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
