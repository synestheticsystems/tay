//! What the environment remembers being asked for, as `tay status` and
//! `tay set` show it: the value of a `--set` of any setting the stack does
//! not declare plain is hidden, since it may be a secret.

use std::path::Path;

use crate::definition::read;
use crate::diagnostic::Diagnostics;
use crate::diagnostic::source::Sources;
use crate::disk::Asked;

/// `asked` as it is shown, for the stack at `root`.
pub fn shown(mut asked: Asked, root: &Path) -> Asked {
    let mut sources = Sources::default();
    let mut d = Diagnostics::default();
    let stack = match read::read(root, &mut sources, &mut d) {
        read::Outcome::Read { stack, .. } => Some(stack),
        read::Outcome::TooOld(_) | read::Outcome::Unreadable => None,
    };
    for set in &mut asked.sets {
        let Some((place, _)) = set.split_once('=') else {
            continue;
        };
        let Some(setting) = place.strip_prefix("settings.") else {
            continue;
        };
        let plain = stack
            .as_ref()
            .and_then(|stack| stack.setting(setting))
            .is_some_and(|declared| !declared.secret);
        if !plain {
            *set = format!("{place}=<secret>");
        }
    }
    asked
}
