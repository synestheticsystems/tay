//! A unit's log, in `.tay/logs/<unit>/`: every line the unit writes, with
//! the time it was written, and tay's own notes among them, such as where
//! each attempt starts and how it ended. The file is rotated by size and the
//! oldest files are dropped, so that a stack left running does not fill the
//! disk.
//!
//! Each line is `TIME | TEXT` for what the unit wrote and `TIME # TEXT` for
//! what tay noted, `TIME` being UTC to the millisecond, as long as any other,
//! so that lines sort by time as their text does.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::clock;
use crate::disk::DiskError;

/// The file being written.
const FILE: &str = "output.log";

/// The size at which the file is set aside and a new one begun.
const ROTATE_AT: u64 = 8 << 20;

/// How many files set aside are kept: `output.log.1`, the newer, and
/// `output.log.2`. So a unit's log holds at most three files, 24 MiB.
const SET_ASIDE: u32 = 2;

/// The longest line kept whole; a longer one is cut into lines this long,
/// so that a unit that never ends a line cannot fill the keeper's memory.
pub const LONGEST_LINE: usize = 64 * 1024;

/// Who wrote a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writer {
    /// The unit, on its standard output or error.
    Unit,
    /// tay, noting what happened to the unit.
    Tay,
}

impl Writer {
    fn marker(self) -> char {
        match self {
            Writer::Unit => '|',
            Writer::Tay => '#',
        }
    }
}

/// One line of a log, as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// When it was written, as the log writes times.
    pub time: String,
    pub writer: Writer,
    pub text: String,
}

impl Line {
    /// The line as the log holds it, without its end.
    pub fn whole(&self) -> String {
        format!("{} {} {}", self.time, self.writer.marker(), self.text)
    }

    /// The line read from its text in a log; `None` for text that is not a
    /// line tay wrote.
    pub fn parse(text: &str) -> Option<Line> {
        let (time, rest) = text.split_once(' ')?;
        let (marker, text) = rest.split_once(' ').unwrap_or((rest, ""));
        let writer = match marker {
            "|" => Writer::Unit,
            "#" => Writer::Tay,
            _ => return None,
        };
        Some(Line {
            time: time.to_string(),
            writer,
            text: text.to_string(),
        })
    }
}

/// The log of one unit: the folder its files are in.
#[derive(Clone, Debug)]
pub struct UnitLog {
    folder: PathBuf,
}

impl UnitLog {
    pub fn at(folder: PathBuf) -> UnitLog {
        UnitLog { folder }
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The file being written.
    pub fn current(&self) -> PathBuf {
        self.folder.join(FILE)
    }

    fn set_aside(&self, number: u32) -> PathBuf {
        self.folder.join(format!("{FILE}.{number}"))
    }

    /// The log, opened to be added to; its folder is made if it is missing.
    pub fn open(&self) -> Result<Adding, DiskError> {
        fs::create_dir_all(&self.folder).map_err(DiskError::io(&self.folder, "made"))?;
        let path = self.current();
        let file = append(&path)?;
        let size = file.metadata().map_err(DiskError::io(&path, "read"))?.len();
        Ok(Adding {
            log: self.clone(),
            file,
            size,
        })
    }

    /// Every line of the log written at `since` or later, oldest first, and
    /// where the file being written was read to, for [`Following`] on from
    /// there.
    pub fn read_since(&self, since: &str) -> Result<(Vec<Line>, Following), DiskError> {
        let mut lines = Vec::new();
        for number in (1..=SET_ASIDE).rev() {
            let path = self.set_aside(number);
            match File::open(&path) {
                Ok(file) => {
                    read_lines(file, since, &mut lines).map_err(DiskError::io(&path, "read"))?
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(DiskError::io(path, "read")(error)),
            }
        }
        let mut following = Following {
            log: self.clone(),
            file: None,
            partial: Vec::new(),
        };
        following.read(since, &mut lines)?;
        Ok((lines, following))
    }

    /// The last `count` lines of the log.
    pub fn last(&self, count: usize) -> Result<Vec<Line>, DiskError> {
        let (mut lines, _) = self.read_since("")?;
        let keep = lines.len().saturating_sub(count);
        Ok(lines.split_off(keep))
    }
}

/// A unit's log open to be added to, set aside when it grows too long.
pub struct Adding {
    log: UnitLog,
    file: File,
    size: u64,
}

impl Adding {
    /// Writes `text`, as `writer` wrote it, with the time now.
    pub fn write(&mut self, writer: Writer, text: &str) -> Result<(), DiskError> {
        self.write_at(clock::now_millis(), writer, text)
    }

    /// Writes `text`, as `writer` wrote it at `millis`, in milliseconds
    /// since 1970: what a container wrote, at the time Docker kept for it.
    pub fn write_at(&mut self, millis: u64, writer: Writer, text: &str) -> Result<(), DiskError> {
        let line = Line {
            time: clock::stamp(millis),
            writer,
            text: text.to_string(),
        };
        let mut bytes = line.whole().into_bytes();
        bytes.push(b'\n');
        if self.size > 0 && self.size + bytes.len() as u64 > ROTATE_AT {
            self.rotate()?;
        }
        let path = self.log.current();
        self.file
            .write_all(&bytes)
            .map_err(DiskError::io(&path, "written"))?;
        self.size += bytes.len() as u64;
        Ok(())
    }

    /// Sets the file aside, dropping the oldest, and begins a new one.
    fn rotate(&mut self) -> Result<(), DiskError> {
        for number in (1..SET_ASIDE).rev() {
            let from = self.log.set_aside(number);
            match fs::rename(&from, self.log.set_aside(number + 1)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(DiskError::io(from, "set aside")(error)),
            }
        }
        let current = self.log.current();
        fs::rename(&current, self.log.set_aside(1))
            .map_err(DiskError::io(&current, "set aside"))?;
        self.file = append(&current)?;
        self.size = 0;
        Ok(())
    }
}

fn append(path: &Path) -> Result<File, DiskError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(DiskError::io(path, "opened"))
}

/// Reads every whole line of `file` written at `since` or later.
fn read_lines(file: File, since: &str, lines: &mut Vec<Line>) -> io::Result<()> {
    for text in BufReader::new(file).split(b'\n') {
        let text = text?;
        if let Some(line) = Line::parse(&String::from_utf8_lossy(&text))
            && line.time.as_str() >= since
        {
            lines.push(line);
        }
    }
    Ok(())
}

/// The file a unit's log is written to, read as it grows: past where it was
/// read to, and on into the next file once it is set aside.
pub struct Following {
    log: UnitLog,
    /// The file being read, once there is one, and the identity it has, to
    /// tell when another file has taken its place.
    file: Option<(File, (u64, u64))>,
    /// A line read up to where the file ends, not yet ended.
    partial: Vec<u8>,
}

impl Following {
    /// Adds to `lines` each line written at `since` or later that the file
    /// has gained since it was last read.
    pub fn read(&mut self, since: &str, lines: &mut Vec<Line>) -> Result<(), DiskError> {
        let path = self.log.current();
        loop {
            let Some((file, identity)) = &mut self.file else {
                match File::open(&path) {
                    Ok(file) => {
                        let metadata = file.metadata().map_err(DiskError::io(&path, "read"))?;
                        self.file = Some((file, (metadata.dev(), metadata.ino())));
                        continue;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                    Err(error) => return Err(DiskError::io(path, "read")(error)),
                }
            };
            let replaced = fs::metadata(&path)
                .map(|metadata| (metadata.dev(), metadata.ino()) != *identity)
                .unwrap_or(false);
            // A file set aside is written no more once another is at its
            // path, so reading it to its end now reads all of it.
            let mut gained = Vec::new();
            file.read_to_end(&mut gained)
                .map_err(DiskError::io(&path, "read"))?;
            self.partial.extend_from_slice(&gained);
            while let Some(end) = self.partial.iter().position(|byte| *byte == b'\n') {
                let text: Vec<u8> = self.partial.drain(..=end).collect();
                if let Some(line) = Line::parse(&String::from_utf8_lossy(&text[..end]))
                    && line.time.as_str() >= since
                {
                    lines.push(line);
                }
            }
            if !replaced {
                return Ok(());
            }
            self.file = None;
            self.partial.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_its_time_who_wrote_it_and_its_text() {
        let line = Line::parse("2026-11-10T06:42:51.005Z | listening on 3000").unwrap();
        assert_eq!(
            (line.time.as_str(), line.writer, line.text.as_str()),
            (
                "2026-11-10T06:42:51.005Z",
                Writer::Unit,
                "listening on 3000"
            )
        );
        assert_eq!(line.whole(), "2026-11-10T06:42:51.005Z | listening on 3000");
        let note = Line::parse("2026-11-10T06:42:51.005Z # attempt 1: started").unwrap();
        assert_eq!(note.writer, Writer::Tay);
        assert_eq!(Line::parse("2026-11-10T06:42:51.005Z |").unwrap().text, "");
        assert_eq!(Line::parse("not a line of a log"), None);
    }

    #[test]
    fn a_log_is_set_aside_when_it_grows_too_long_and_the_oldest_is_dropped() {
        let folder = tempfile::tempdir().unwrap();
        let log = UnitLog::at(folder.path().join("web"));
        let mut adding = log.open().unwrap();
        let long = "x".repeat(LONGEST_LINE);
        // Enough for the file to be set aside more often than files are kept.
        let lines = (ROTATE_AT as usize / LONGEST_LINE) * 4;
        for number in 0..lines {
            adding
                .write(Writer::Unit, &format!("{number} {long}"))
                .unwrap();
        }
        let names = |folder: &Path| {
            let mut names: Vec<String> = fs::read_dir(folder)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        assert_eq!(
            names(&folder.path().join("web")),
            ["output.log", "output.log.1", "output.log.2"]
        );
        for file in names(&folder.path().join("web")) {
            let size = fs::metadata(folder.path().join("web").join(file))
                .unwrap()
                .len();
            assert!(size <= ROTATE_AT, "{size}");
        }
        // What is kept is the newest, in order, ending with the last line.
        let (kept, _) = log.read_since("").unwrap();
        assert!(kept.len() < lines);
        let last = format!("{} ", lines - 1);
        assert!(kept.last().unwrap().text.starts_with(&last));
        let numbers: Vec<usize> = kept
            .iter()
            .map(|line| line.text.split(' ').next().unwrap().parse().unwrap())
            .collect();
        assert!(numbers.windows(2).all(|pair| pair[1] == pair[0] + 1));
    }

    #[test]
    fn following_a_log_reads_each_new_line_once_and_on_into_the_next_file() {
        let folder = tempfile::tempdir().unwrap();
        let log = UnitLog::at(folder.path().join("web"));
        let mut adding = log.open().unwrap();
        adding.write(Writer::Unit, "first").unwrap();
        let (lines, mut following) = log.read_since("").unwrap();
        assert_eq!(lines.len(), 1);
        adding.write(Writer::Unit, "second").unwrap();
        adding.rotate().unwrap();
        adding.write(Writer::Tay, "third").unwrap();
        let mut more = Vec::new();
        following.read("", &mut more).unwrap();
        let texts: Vec<&str> = more.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["second", "third"]);
        let mut none = Vec::new();
        following.read("", &mut none).unwrap();
        assert!(none.is_empty());
        assert_eq!(log.last(2).unwrap().len(), 2);
    }
}
