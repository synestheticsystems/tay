//! What the environment remembers being asked for, as `tay status` and
//! `tay set` show it: the value of a `--set` of any setting the stack does
//! not declare plain is hidden, since it may be a secret.

use std::path::Path;

use crate::definition::{Stack, read};
use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Sources;
use crate::disk::Asked;

/// The definitions of the stack at `root`, as far as they can be read,
/// which say which settings are secret; `None` when they cannot be read at
/// all.
pub fn stack(root: &Path) -> Option<Box<Stack>> {
    let mut sources = Sources::default();
    let mut d = Diagnostics::default();
    match read::read(root, &mut sources, &mut d) {
        read::Outcome::Read { stack, .. } => Some(stack),
        read::Outcome::TooOld(_) | read::Outcome::Unreadable => None,
    }
}

/// `asked` as it is shown, for `stack`: with every setting hidden when its
/// definitions could not be read.
pub fn shown(mut asked: Asked, stack: Option<&Stack>) -> Asked {
    for set in &mut asked.sets {
        let Some((place, _)) = set.split_once('=') else {
            continue;
        };
        let Some(setting) = place.strip_prefix("settings.") else {
            continue;
        };
        let plain = stack
            .and_then(|stack| stack.setting(setting))
            .is_some_and(|declared| !declared.secret);
        if !plain {
            *set = format!("{place}=<secret>");
        }
    }
    asked
}
