//! Whether an environment is meant to be up: the one fact about intent that
//! tay keeps on disk, so that a daemon started after another died knows
//! whether to find the environment's units again.

use std::io;

use serde::{Deserialize, Serialize};

use super::Layout;
use crate::disk::{DiskError, write_whole};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Intent {
    Up,
    Down,
}

impl Intent {
    /// The environment's intent; `Down` when nothing has said it is meant
    /// to be up.
    pub fn read(layout: &Layout) -> Result<Intent, DiskError> {
        let path = layout.intent();
        match std::fs::read_to_string(&path) {
            Ok(text) => match text.trim() {
                "up" => Ok(Intent::Up),
                "down" => Ok(Intent::Down),
                other => Err(DiskError::Unexpected {
                    path,
                    found: other.to_string(),
                    wanted: "`up` or `down`",
                }),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Intent::Down),
            Err(error) => Err(DiskError::io(path, "read")(error)),
        }
    }

    pub fn write(self, layout: &Layout) -> Result<(), DiskError> {
        layout.make_state()?;
        write_whole(
            &layout.intent(),
            format!("{}\n", self.word()).as_bytes(),
            0o644,
        )
    }

    pub fn word(self) -> &'static str {
        match self {
            Intent::Up => "up",
            Intent::Down => "down",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_environment_is_down_until_it_is_said_to_be_up() {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::of(root.path());
        assert_eq!(Intent::read(&layout).unwrap(), Intent::Down);
        Intent::Up.write(&layout).unwrap();
        assert_eq!(Intent::read(&layout).unwrap(), Intent::Up);
        assert_eq!(std::fs::read_to_string(layout.intent()).unwrap(), "up\n");
        Intent::Down.write(&layout).unwrap();
        assert_eq!(Intent::read(&layout).unwrap(), Intent::Down);
    }
}
