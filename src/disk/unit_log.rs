//! A unit's log, in `.tay/logs/<unit>/`: every line the unit writes, with
//! the time it was written, and tay's own notes among them, such as where
//! each attempt starts and how it ended. The file is rotated by size and the
//! oldest files are dropped, so that a stack left running does not fill the
//! disk.
//!
//! Each line is `TIME | TEXT` for what the unit wrote and `TIME # TEXT` for
//! what tay noted, `TIME` being UTC to the millisecond, as long as any other,
//! so that lines sort by time as their text does.
//!
//! A line that cannot be written, as when the disk is full, is lost, and
//! whoever writes the unit's lines goes on, as the unit does: a log that
//! cannot be written is no reason to stop what it is the log of. Once a
//! line can be written again, a line of tay's own before it says how many
//! were lost.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

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

/// The lines each log, by its folder, lost to a writer that was dropped
/// before it could say so, as one is when its unit is stopped while the
/// disk is full: the next writer of that log in this process says them
/// with its own.
static UNSAID: Mutex<BTreeMap<PathBuf, u64>> = Mutex::new(BTreeMap::new());

/// Who wrote a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writer {
    /// The unit, on its standard output or error.
    Unit,
    /// tay, noting what happened to the unit.
    Tay,
}

impl Writer {
    /// What a line's writer is written as, between its time and its text.
    pub fn marker(self) -> char {
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
        let lost = unsaid().remove(&self.folder).unwrap_or(0);
        Ok(Adding {
            log: self.clone(),
            file,
            size,
            lost,
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

    /// The log followed from where the file being written ends now: what
    /// is written to it from now on.
    pub fn follow_from_now(&self) -> Result<Following, DiskError> {
        let path = self.current();
        let file = match File::open(&path) {
            Ok(mut file) => {
                let metadata = file.metadata().map_err(DiskError::io(&path, "read"))?;
                file.seek(SeekFrom::End(0))
                    .map_err(DiskError::io(&path, "read"))?;
                Some((file, (metadata.dev(), metadata.ino())))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(DiskError::io(path, "read")(error)),
        };
        Ok(Following {
            log: self.clone(),
            file,
            partial: Vec::new(),
        })
    }

    /// The last `count` lines of the log.
    pub fn last(&self, count: usize) -> Result<Vec<Line>, DiskError> {
        let (mut lines, _) = self.read_since("")?;
        let keep = lines.len().saturating_sub(count);
        Ok(lines.split_off(keep))
    }

    /// Whether the log has a file: the one being written, or one set aside.
    fn has_files(&self) -> bool {
        std::iter::once(self.current())
            .chain((1..=SET_ASIDE).map(|number| self.set_aside(number)))
            .any(|path| path.exists())
    }
}

/// The units that have a log in `logs`, an environment's folder of logs,
/// by name: each folder there that holds a unit's log files, which the
/// daemon's own folder does not.
pub fn units_with_logs(logs: &Path) -> Result<Vec<String>, DiskError> {
    let entries = match fs::read_dir(logs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(DiskError::io(logs, "read")(error)),
    };
    let mut units = Vec::new();
    for entry in entries {
        let entry = entry.map_err(DiskError::io(logs, "read"))?;
        let Some(unit) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if UnitLog::at(entry.path()).has_files() {
            units.push(unit);
        }
    }
    units.sort();
    Ok(units)
}

/// A unit's log open to be added to, set aside when it grows too long.
pub struct Adding {
    log: UnitLog,
    file: File,
    size: u64,
    /// The lines that could not be written since one last was.
    lost: u64,
}

impl Adding {
    /// Writes `text`, as `writer` wrote it, with the time now, or loses it.
    pub fn write(&mut self, writer: Writer, text: &str) {
        self.write_at(clock::now_millis(), writer, text);
    }

    /// Writes `text`, as `writer` wrote it at `millis`, in milliseconds
    /// since 1970: what a container wrote, at the time Docker kept for it.
    /// A line that cannot be written is lost; the first written after
    /// lines were lost follows a line of tay's own, of the same time, that
    /// says how many.
    pub fn write_at(&mut self, millis: u64, writer: Writer, text: &str) {
        if self.lost > 0 {
            // A line that could not be written may have been written in
            // part: the note begins a line of its own.
            let mut note = vec![b'\n'];
            note.extend(bytes_of(millis, Writer::Tay, &note_of_lost(self.lost)));
            if self.put(&note).is_err() {
                self.lost += 1;
                return;
            }
            self.lost = 0;
        }
        if self.put(&bytes_of(millis, writer, text)).is_err() {
            self.lost += 1;
        }
    }

    /// Adds `bytes` to the file, setting it aside first if they would make
    /// it too long.
    fn put(&mut self, bytes: &[u8]) -> Result<(), DiskError> {
        if self.size > 0 && self.size + bytes.len() as u64 > ROTATE_AT {
            self.rotate()?;
        }
        let path = self.log.current();
        self.file
            .write_all(bytes)
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
        match fs::rename(&current, self.log.set_aside(1)) {
            Ok(()) => {}
            // Set aside already, by a setting aside that could not begin
            // the new file, as on a full disk.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(DiskError::io(current, "set aside")(error)),
        }
        self.file = append(&current)?;
        self.size = 0;
        Ok(())
    }
}

impl Drop for Adding {
    /// Leaves the lines it lost and has not said to the next writer of the
    /// log.
    fn drop(&mut self) {
        if self.lost > 0 {
            *unsaid().entry(self.log.folder.clone()).or_default() += self.lost;
        }
    }
}

/// The line `text`, as `writer` wrote it at `millis`, as the log holds it,
/// with its end.
fn bytes_of(millis: u64, writer: Writer, text: &str) -> Vec<u8> {
    let line = Line {
        time: clock::stamp(millis),
        writer,
        text: text.to_string(),
    };
    let mut bytes = line.whole().into_bytes();
    bytes.push(b'\n');
    bytes
}

/// What a log says of `lost` lines that could not be written to it.
fn note_of_lost(lost: u64) -> String {
    match lost {
        1 => "1 line could not be written to this log, and is lost".to_string(),
        lost => format!("{lost} lines could not be written to this log, and are lost"),
    }
}

/// The lines lost and not said, by log, held. A count is whole whatever
/// panicked while it was held.
fn unsaid() -> MutexGuard<'static, BTreeMap<PathBuf, u64>> {
    UNSAID.lock().unwrap_or_else(PoisonError::into_inner)
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
    use std::os::unix::fs::PermissionsExt;

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
            adding.write(Writer::Unit, &format!("{number} {long}"));
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
        adding.write(Writer::Unit, "first");
        let (lines, mut following) = log.read_since("").unwrap();
        assert_eq!(lines.len(), 1);
        adding.write(Writer::Unit, "second");
        adding.rotate().unwrap();
        adding.write(Writer::Tay, "third");
        let mut more = Vec::new();
        following.read("", &mut more).unwrap();
        let texts: Vec<&str> = more.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["second", "third"]);
        let mut none = Vec::new();
        following.read("", &mut none).unwrap();
        assert!(none.is_empty());
        assert_eq!(log.last(2).unwrap().len(), 2);
    }

    #[test]
    fn following_from_now_reads_only_what_is_written_after() {
        let folder = tempfile::tempdir().unwrap();
        let log = UnitLog::at(folder.path().join("seed"));
        let mut new = log.follow_from_now().unwrap();
        let mut adding = log.open().unwrap();
        adding.write(Writer::Unit, "first");
        let mut following = log.follow_from_now().unwrap();
        adding.write(Writer::Unit, "second");
        let read = |following: &mut Following| {
            let mut lines = Vec::new();
            following.read("", &mut lines).unwrap();
            lines.into_iter().map(|line| line.text).collect::<Vec<_>>()
        };
        assert_eq!(read(&mut following), ["second"]);
        // Followed before the log had a file, it reads the file from its
        // start once there is one.
        assert_eq!(read(&mut new), ["first", "second"]);
    }

    #[test]
    fn the_units_with_logs_are_the_folders_that_hold_a_units_log_files() {
        let folder = tempfile::tempdir().unwrap();
        let logs = folder.path().join("logs");
        assert!(units_with_logs(&logs).unwrap().is_empty());
        for unit in ["web", "api"] {
            UnitLog::at(logs.join(unit)).open().unwrap();
        }
        // A log whose file being written was set aside, and the daemon's
        // own folder, which holds no unit's log.
        fs::create_dir_all(logs.join("set-aside")).unwrap();
        fs::write(logs.join("set-aside").join("output.log.1"), "").unwrap();
        fs::create_dir_all(logs.join("tay")).unwrap();
        fs::write(logs.join("tay").join("daemon.log"), "").unwrap();
        assert_eq!(units_with_logs(&logs).unwrap(), ["api", "set-aside", "web"]);
    }

    /// A folder that takes no new file, as a full disk takes none, until
    /// it is dropped.
    struct Refusing<'a>(&'a Path);

    impl Refusing<'_> {
        fn of(folder: &Path) -> Refusing<'_> {
            fs::set_permissions(folder, fs::Permissions::from_mode(0o555)).unwrap();
            Refusing(folder)
        }
    }

    impl Drop for Refusing<'_> {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.0, fs::Permissions::from_mode(0o755));
        }
    }

    /// Who wrote each line of `log`, and its text.
    fn lines(log: &UnitLog) -> Vec<(Writer, String)> {
        let (lines, _) = log.read_since("").unwrap();
        lines
            .into_iter()
            .map(|line| (line.writer, line.text))
            .collect()
    }

    #[test]
    fn a_line_that_cannot_be_written_is_lost_and_the_next_written_says_how_many_were() {
        let folder = tempfile::tempdir().unwrap();
        let log = UnitLog::at(folder.path().join("web"));
        let mut adding = log.open().unwrap();
        adding.write(Writer::Unit, "first");
        // The file is as long as it may grow, so the next line sets it
        // aside, which its folder refuses.
        adding.size = ROTATE_AT;
        let refusing = Refusing::of(log.folder());
        adding.write(Writer::Unit, "second");
        adding.write(Writer::Tay, "third");
        drop(refusing);
        adding.write(Writer::Unit, "fourth");
        adding.write(Writer::Unit, "fifth");
        let said = "2 lines could not be written to this log, and are lost";
        assert_eq!(
            lines(&log),
            [
                (Writer::Unit, "first".to_string()),
                (Writer::Tay, said.to_string()),
                (Writer::Unit, "fourth".to_string()),
                (Writer::Unit, "fifth".to_string()),
            ]
        );
    }

    #[test]
    fn a_setting_aside_that_could_not_begin_its_new_file_is_finished_by_the_next_line() {
        let folder = tempfile::tempdir().unwrap();
        let log = UnitLog::at(folder.path().join("web"));
        let mut adding = log.open().unwrap();
        adding.write(Writer::Unit, "first");
        // As a full disk leaves it: the file set aside, and no new one.
        fs::rename(log.current(), log.set_aside(1)).unwrap();
        adding.size = ROTATE_AT;
        adding.write(Writer::Unit, "second");
        assert_eq!(
            lines(&log),
            [
                (Writer::Unit, "first".to_string()),
                (Writer::Unit, "second".to_string()),
            ]
        );
    }

    #[test]
    fn lines_lost_by_a_writer_dropped_before_it_could_say_so_are_said_by_the_next() {
        let folder = tempfile::tempdir().unwrap();
        let log = UnitLog::at(folder.path().join("web"));
        let mut first = log.open().unwrap();
        first.size = ROTATE_AT;
        let refusing = Refusing::of(log.folder());
        first.write(Writer::Unit, "lost");
        drop(first);
        drop(refusing);
        log.open().unwrap().write(Writer::Unit, "written");
        assert_eq!(
            lines(&log),
            [
                (
                    Writer::Tay,
                    "1 line could not be written to this log, and is lost".to_string()
                ),
                (Writer::Unit, "written".to_string()),
            ]
        );
    }

    #[test]
    fn what_was_written_of_a_lost_line_does_not_take_the_next_line_with_it() {
        let folder = tempfile::tempdir().unwrap();
        let log = UnitLog::at(folder.path().join("web"));
        let mut adding = log.open().unwrap();
        // As a full disk leaves a line it took only the start of.
        adding
            .file
            .write_all(b"2026-11-10T06:42:51.005Z | the sta")
            .unwrap();
        adding.lost = 1;
        adding.write(Writer::Unit, "next");
        assert_eq!(
            lines(&log),
            [
                (Writer::Unit, "the sta".to_string()),
                (
                    Writer::Tay,
                    "1 line could not be written to this log, and is lost".to_string()
                ),
                (Writer::Unit, "next".to_string()),
            ]
        );
    }
}
