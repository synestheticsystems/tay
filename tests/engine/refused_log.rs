//! A unit's log made to refuse lines, as a full disk refuses them, and what
//! it says once it takes them again: for the tests of host processes and of
//! containers, whose `talker` units print `line N` once told to.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::harness::eventually;

/// The length at which a unit's log file is set aside and a new one begun.
const SET_ASIDE_AT: u64 = 8 << 20;

/// What a line of a log is: one the unit printed, by its number, or tay's
/// note of how many lines were lost.
#[derive(Debug, PartialEq)]
pub enum Said {
    Printed(u64),
    Lost(u64),
}

/// The folder of `unit`'s log, with its file made, before the unit is
/// first brought up, nearly as long as it may grow: what tay notes as the
/// unit starts fits, and a few lines it prints then set it aside.
pub fn nearly_full(root: &Path, unit: &str) -> PathBuf {
    let folder = root.join(".tay/logs").join(unit);
    fs::create_dir_all(&folder).unwrap();
    let mut file = fs::File::create(folder.join("output.log")).unwrap();
    file.set_len(SET_ASIDE_AT - 1025).unwrap();
    file.write_all(b"\n").unwrap();
    folder
}

/// A folder that takes no new file, as a full disk takes none, until it is
/// dropped: a log in it whose file must be set aside cannot take a line.
pub struct Refusing(PathBuf);

impl Refusing {
    pub fn of(folder: &Path) -> Refusing {
        fs::set_permissions(folder, fs::Permissions::from_mode(0o555)).unwrap();
        Refusing(folder.to_path_buf())
    }
}

impl Drop for Refusing {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

/// What `unit`'s log holds, across its files, in the order written.
pub fn said(root: &Path, unit: &str) -> Vec<Said> {
    let folder = root.join(".tay/logs").join(unit);
    let text = ["output.log.1", "output.log"]
        .iter()
        .map(|file| fs::read_to_string(folder.join(file)).unwrap_or_default())
        .collect::<String>();
    text.lines()
        .filter_map(|line| {
            if let Some((_, number)) = line.split_once(" | line ") {
                return Some(Said::Printed(number.parse().unwrap()));
            }
            let (_, note) = line.split_once(" # ")?;
            let (lost, rest) = note.split_once(' ')?;
            rest.contains("could not be written to this log")
                .then(|| Said::Lost(lost.parse().unwrap()))
        })
        .collect()
}

/// Waits until `unit`'s log says how many lines it lost, and holds a line
/// after that; what it holds then.
pub fn said_lost(root: &Path, unit: &str) -> Vec<Said> {
    let mut held = Vec::new();
    let found = eventually(Duration::from_secs(60), || {
        held = said(root, unit);
        held.iter()
            .position(|said| matches!(said, Said::Lost(_)))
            .is_some_and(|lost| lost + 1 < held.len())
    });
    assert!(found, "the log never said what it lost: {held:?}");
    held
}

/// Checks that `held`, what the log of a unit that printed without a break
/// holds, says once how many lines were lost, and that the count is of
/// exactly the lines missing between the last written before the log
/// refused them and the first written after.
pub fn lost_once_and_counted(held: &[Said]) {
    let notes: Vec<usize> = (0..held.len())
        .filter(|at| matches!(held[*at], Said::Lost(_)))
        .collect();
    let [at] = notes[..] else {
        panic!("the log says once what it lost: {held:?}");
    };
    let (Said::Lost(lost), Said::Printed(next)) = (&held[at], &held[at + 1]) else {
        panic!("a line follows the note: {held:?}");
    };
    let before = match at.checked_sub(1).map(|before| &held[before]) {
        Some(Said::Printed(number)) => *number,
        _ => 0,
    };
    assert!(*lost > 0, "{held:?}");
    assert_eq!(before + lost + 1, *next, "{held:?}");
    let numbers: Vec<u64> = (held.iter())
        .filter_map(|said| match said {
            Said::Printed(number) => Some(*number),
            Said::Lost(_) => None,
        })
        .collect();
    assert!(
        (numbers.windows(2)).all(|pair| pair[1] == pair[0] + 1 || pair[0] == before),
        "{held:?}"
    );
}
