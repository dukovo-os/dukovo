// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Reads the records of the log for logforward(8) from the day files logd
//! keeps, from a destination's position on, a batch at a time (ADR 0482).
//!
//! logd only appends to the files, one line of JSON for each record, and
//! logtrim(8) removes the oldest whole days, so the files are the queue of
//! each destination. logd appends a record to the file of its own day,
//! which need not be the newest file, so each batch looks at every day
//! file, oldest first, and reads what each has gained since its offset in
//! the position. A batch holds whole lines only: the last line of a file
//! may be cut short while logd writes it, and it is left until the rest is
//! there, or, with no change to the file for [`ABANDONED`], until it is
//! plain that logd stopped in the middle of it. A line that is not a
//! record is passed over and counted. A day file trimmed before all of it
//! was sent is said in the log.

use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::position::Position;
use crate::logd::{self, Record};

/// The most lines a batch holds.
const BATCH_LINES: usize = 256;
/// The most bytes of the log a batch reads, unless one line is longer.
const BATCH_BYTES: usize = 64 << 10;
/// How much of the end of a file is searched for its last whole line.
const TAIL: u64 = 64 << 10;
/// How long a file must stay unchanged before its last line without an
/// end is taken as whole: logd writes a line with one write, so only a
/// logd that ended in the middle of one leaves it so.
const ABANDONED: Duration = Duration::from_secs(60);

/// What one read of the log gave.
#[derive(Debug, PartialEq, Eq)]
pub struct Batch {
    /// The records of the lines read, in order.
    pub records: Vec<Record>,
    /// How many lines were read, the unreadable among them: 0 when there
    /// was nothing new.
    pub lines: usize,
    /// Where the reader is after the batch.
    pub position: Position,
    /// What logforward says in the log of the batch, at warning level.
    pub notes: Vec<String>,
}

/// How much a batch may still take.
struct Room {
    lines: usize,
    bytes: usize,
}

impl Room {
    fn is_full(&self) -> bool {
        self.lines == 0 || self.bytes == 0
    }
}

/// A destination's place in the log.
pub struct Reader {
    dir: PathBuf,
    /// The destination's name, for what is said of it.
    name: String,
    position: Position,
    /// Each day file's length when last listed, to tell a trimmed file
    /// that was not all sent.
    lengths: BTreeMap<String, u64>,
}

impl Reader {
    /// A reader of the log in `dir` for destination `name`, starting at
    /// `position`.
    pub fn new(dir: &Path, name: &str, position: Position) -> Reader {
        Reader {
            dir: dir.to_path_buf(),
            name: name.to_string(),
            position,
            lengths: BTreeMap::new(),
        }
    }

    /// Where the next batch starts.
    pub fn position(&self) -> &Position {
        &self.position
    }

    /// Goes back to `position`, after a failure to send.
    pub fn seek(&mut self, position: Position) {
        self.position = position;
    }

    /// The next batch of up to 256 lines or 64 KiB, which holds no line
    /// that is not whole yet.
    ///
    /// # Errors
    ///
    /// The file system's, when a day file cannot be read; the reader then
    /// stays where it was.
    pub fn read_batch(&mut self) -> io::Result<Batch> {
        let files = day_files(&self.dir);
        let names: Vec<String> = files.iter().map(|(name, _)| name.clone()).collect();
        let mut notes = Vec::new();
        for (file, offset) in self.position.keep_only(&names) {
            if self.lengths.get(&file).is_some_and(|length| *length > offset) {
                notes.push(format!(
                    "{}: {file} was trimmed before all of it could be sent",
                    self.name
                ));
            }
        }
        self.lengths =
            files.iter().map(|(name, metadata)| (name.clone(), metadata.len())).collect();
        // The position moves only once the whole batch is read, so that a
        // failed read sends nothing twice and skips nothing.
        let mut position = self.position.clone();
        let mut batch =
            Batch { records: Vec::new(), lines: 0, position: Position::default(), notes };
        let mut room = Room { lines: BATCH_LINES, bytes: BATCH_BYTES };
        for (name, metadata) in &files {
            if room.is_full() {
                break;
            }
            let mut offset = position.offset(name);
            if metadata.len() == offset {
                continue;
            }
            if metadata.len() < offset {
                // The file was made again since, as when the log was wiped.
                batch.notes.push(format!(
                    "{}: {name} is shorter than the position in it; sending it again from its \
                     start",
                    self.name
                ));
                offset = 0;
            }
            let abandoned = metadata
                .modified()
                .ok()
                .and_then(|changed| SystemTime::now().duration_since(changed).ok())
                .is_some_and(|unchanged| unchanged >= ABANDONED);
            let read = self.read_file(name, offset, abandoned, &mut room, &mut batch)?;
            position.set(name, read);
        }
        self.position = position;
        batch.position = self.position.clone();
        Ok(batch)
    }

    /// Reads whole lines of day file `name` from byte `offset` into
    /// `batch` while there is `room`, and gives the offset after them. A
    /// line without its end is left for later, unless the file was
    /// `abandoned` with it.
    fn read_file(
        &self,
        name: &str,
        mut offset: u64,
        abandoned: bool,
        room: &mut Room,
        batch: &mut Batch,
    ) -> io::Result<u64> {
        let file = match File::open(self.dir.join(name)) {
            Ok(file) => file,
            // logtrim removed it since the folder was listed.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(offset),
            Err(error) => return Err(error),
        };
        let mut lines = BufReader::new(file);
        lines.seek(SeekFrom::Start(offset))?;
        let (mut unreadable, mut line) = (0u64, Vec::new());
        while !room.is_full() {
            line.clear();
            let read = lines.read_until(b'\n', &mut line)?;
            if read == 0 || (line.last() != Some(&b'\n') && !abandoned) {
                break;
            }
            offset += read as u64;
            room.lines -= 1;
            room.bytes = room.bytes.saturating_sub(read);
            batch.lines += 1;
            match serde_json::from_slice::<Record>(&line) {
                Ok(record) => batch.records.push(record),
                Err(_) => unreadable += 1,
            }
        }
        if unreadable > 0 {
            batch.notes.push(format!(
                "{}: passed over {unreadable} unreadable {} in {name}",
                self.name,
                if unreadable == 1 { "line" } else { "lines" },
            ));
        }
        Ok(offset)
    }
}

/// The day files in `dir`, oldest first, with what is known of each.
fn day_files(dir: &Path) -> Vec<(String, Metadata)> {
    logd::day_files(dir)
        .iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            Some((name, fs::metadata(path).ok()?))
        })
        .collect()
}

/// Where a destination that is new starts: in each day file in `dir`,
/// after its last whole line, so that it is sent the log from now on.
pub fn end_of_log(dir: &Path) -> Position {
    let mut position = Position::default();
    for (name, metadata) in day_files(dir) {
        if let Some(end) = last_whole_line(&dir.join(&name), metadata.len()) {
            position.set(&name, end);
        }
    }
    position
}

/// The byte after the last whole line of the file at `path`, which is
/// `len` bytes long.
fn last_whole_line(path: &Path, len: u64) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let start = len.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.take(TAIL).read_to_end(&mut tail).ok()?;
    // Without a newline in the tail, the line there is the one logd is
    // still writing, and the destination starts at its beginning; a line
    // longer than the tail is the start's, which is then passed over as
    // unreadable.
    let whole = tail.iter().rposition(|byte| *byte == b'\n').map_or(0, |at| at + 1);
    Some(start + whole as u64)
}

/// How many bytes of the log in `dir` lie after `position`.
pub fn behind(dir: &Path, position: &Position) -> u64 {
    day_files(dir)
        .iter()
        .map(|(name, metadata)| metadata.len().saturating_sub(position.offset(name)))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    /// A folder of its own for each test.
    fn folder(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("logforward-reader-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn line(seq: u64, text: &str) -> String {
        let record = Record {
            time: "2026-11-13T10:30:00.000000Z".into(),
            boot: "6c1f".into(),
            seq,
            level: "info".into(),
            from: "service".into(),
            name: "sshd".into(),
            text: text.into(),
            ..Record::default()
        };
        format!("{}\n", serde_json::to_string(&record).unwrap())
    }

    fn at(file: &str, offset: u64) -> Position {
        Position::of(file, offset)
    }

    /// The position at each of `offsets`, a day file and the byte in it.
    fn at_each(offsets: &[(&str, u64)]) -> Position {
        let mut position = Position::default();
        for (file, offset) in offsets {
            position.set(file, *offset);
        }
        position
    }

    /// Makes the file at `path` look unchanged for `ago`.
    fn changed_ago(path: &Path, ago: Duration) {
        let file = fs::OpenOptions::new().append(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - ago).unwrap();
    }

    fn texts(batch: &Batch) -> Vec<&str> {
        batch.records.iter().map(|record| record.text.as_str()).collect()
    }

    fn append(dir: &Path, file: &str, text: &str) {
        let mut file =
            fs::OpenOptions::new().create(true).append(true).open(dir.join(file)).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn a_batch_holds_the_lines_after_the_position_and_says_where_it_ends() {
        let dir = folder("whole");
        let (first, second) = (line(1, "one"), line(2, "two"));
        append(&dir, "2026-11-13.jsonl", &format!("{first}{second}"));
        let mut reader = Reader::new(&dir, "siem", at("2026-11-13.jsonl", first.len() as u64));
        let batch = reader.read_batch().unwrap();
        assert_eq!(texts(&batch), ["two"]);
        assert_eq!(batch.lines, 1);
        assert_eq!(batch.position, at("2026-11-13.jsonl", (first.len() + second.len()) as u64));
        assert_eq!(reader.position(), &batch.position);
        assert!(batch.notes.is_empty());
        let again = reader.read_batch().unwrap();
        assert_eq!((again.records.len(), again.lines), (0, 0));
        assert_eq!(again.position, batch.position);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_line_logd_has_not_finished_is_left_until_it_is_whole() {
        let dir = folder("cut");
        let (first, second) = (line(1, "one"), line(2, "two"));
        let cut = second.len() / 2;
        append(&dir, "2026-11-13.jsonl", &format!("{first}{}", &second[..cut]));
        let mut reader = Reader::new(&dir, "siem", at("2026-11-13.jsonl", 0));
        let batch = reader.read_batch().unwrap();
        assert_eq!(texts(&batch), ["one"]);
        assert_eq!(batch.position, at("2026-11-13.jsonl", first.len() as u64));
        assert!(batch.notes.is_empty());
        append(&dir, "2026-11-13.jsonl", &second[cut..]);
        let batch = reader.read_batch().unwrap();
        assert_eq!(texts(&batch), ["two"]);
        assert_eq!(batch.position, at("2026-11-13.jsonl", (first.len() + second.len()) as u64));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn at_the_end_of_a_day_the_next_days_file_goes_on_from_its_start() {
        let dir = folder("days");
        let (one, two) = (line(1, "late"), line(2, "early"));
        append(&dir, "2026-11-13.jsonl", &one);
        let mut reader = Reader::new(&dir, "siem", at("2026-11-13.jsonl", 0));
        assert_eq!(texts(&reader.read_batch().unwrap()), ["late"]);
        append(&dir, "2026-11-15.jsonl", &two);
        let batch = reader.read_batch().unwrap();
        assert_eq!(texts(&batch), ["early"]);
        assert_eq!(
            batch.position,
            at_each(&[
                ("2026-11-13.jsonl", one.len() as u64),
                ("2026-11-15.jsonl", two.len() as u64)
            ])
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_batch_goes_across_days_and_reads_what_the_old_file_gained_before_the_new_one() {
        let dir = folder("across");
        let (one, two, three) = (line(1, "a"), line(2, "b"), line(3, "c"));
        append(&dir, "2026-11-13.jsonl", &format!("{one}{two}"));
        append(&dir, "2026-11-14.jsonl", &three);
        let mut reader = Reader::new(&dir, "siem", at("2026-11-13.jsonl", one.len() as u64));
        let batch = reader.read_batch().unwrap();
        assert_eq!(texts(&batch), ["b", "c"]);
        assert_eq!(batch.lines, 2);
        assert_eq!(
            batch.position,
            at_each(&[
                ("2026-11-13.jsonl", (one.len() + two.len()) as u64),
                ("2026-11-14.jsonl", three.len() as u64)
            ])
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_line_that_reaches_an_older_days_file_after_a_newer_one_began_is_still_sent() {
        let dir = folder("late");
        let (one, two, late) = (line(1, "before midnight"), line(2, "after"), line(3, "late"));
        append(&dir, "2026-11-13.jsonl", &one);
        append(&dir, "2026-11-14.jsonl", &two);
        let mut reader = Reader::new(&dir, "siem", Position::default());
        assert_eq!(texts(&reader.read_batch().unwrap()), ["before midnight", "after"]);
        // Stamped before midnight, it came after the next day's first line;
        // or the clock was set back.
        append(&dir, "2026-11-13.jsonl", &late);
        let batch = reader.read_batch().unwrap();
        assert_eq!(texts(&batch), ["late"]);
        assert_eq!(batch.position.offset("2026-11-13.jsonl"), (one.len() + late.len()) as u64);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_newer_than_the_clock_holds_no_older_file_back() {
        // The clock read from the hardware was a day ahead, then corrected.
        let dir = folder("ahead");
        let (wrong, right) = (line(1, "ahead"), line(2, "corrected"));
        append(&dir, "2026-11-14.jsonl", &wrong);
        let mut reader = Reader::new(&dir, "siem", Position::default());
        assert_eq!(texts(&reader.read_batch().unwrap()), ["ahead"]);
        append(&dir, "2026-11-13.jsonl", &right);
        assert_eq!(texts(&reader.read_batch().unwrap()), ["corrected"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_last_line_without_its_end_passes_once_its_file_stays_unchanged_a_minute() {
        let dir = folder("abandoned");
        let one = line(1, "a");
        let cut = format!("{one}{{\"time\":\"cut");
        append(&dir, "2026-11-13.jsonl", &cut);
        let mut reader = Reader::new(&dir, "siem", at("2026-11-13.jsonl", 0));
        let batch = reader.read_batch().unwrap();
        assert_eq!((texts(&batch), batch.lines), (vec!["a"], 1));
        changed_ago(&dir.join("2026-11-13.jsonl"), ABANDONED);
        let batch = reader.read_batch().unwrap();
        assert_eq!((batch.records.len(), batch.lines), (0, 1));
        assert_eq!(batch.position, at("2026-11-13.jsonl", cut.len() as u64));
        assert_eq!(batch.notes, ["siem: passed over 1 unreadable line in 2026-11-13.jsonl"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_line_that_is_no_record_is_passed_over_and_counted_for_its_file() {
        let dir = folder("unreadable");
        let text = format!(
            "{}not json\n\n{}{{\"time\": 5}}\n{}",
            line(1, "a"),
            line(2, "b"),
            line(3, "c")
        );
        append(&dir, "2026-11-13.jsonl", &text);
        let mut reader = Reader::new(&dir, "siem", at("2026-11-13.jsonl", 0));
        let batch = reader.read_batch().unwrap();
        assert_eq!(texts(&batch), ["a", "b", "c"]);
        assert_eq!(batch.lines, 6);
        assert_eq!(batch.position.offset("2026-11-13.jsonl"), text.len() as u64);
        assert_eq!(batch.notes, ["siem: passed over 3 unreadable lines in 2026-11-13.jsonl"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_day_file_trimmed_before_all_of_it_was_sent_is_said_and_forgotten() {
        let dir = folder("trimmed");
        let (old, kept) = (line(1, "old"), line(2, "kept"));
        append(&dir, "2026-11-10.jsonl", &format!("{old}{old}"));
        append(&dir, "2026-11-13.jsonl", &kept);
        let mut reader = Reader::new(&dir, "siem", Position::default());
        assert_eq!(texts(&reader.read_batch().unwrap()), ["old", "old", "kept"]);
        // The server never took the batch, so the reader goes back.
        reader.seek(at("2026-11-10.jsonl", old.len() as u64));
        fs::remove_file(dir.join("2026-11-10.jsonl")).unwrap();
        let batch = reader.read_batch().unwrap();
        assert_eq!(texts(&batch), ["kept"]);
        assert_eq!(batch.position, at("2026-11-13.jsonl", kept.len() as u64));
        assert_eq!(
            batch.notes,
            ["siem: 2026-11-10.jsonl was trimmed before all of it could be sent"]
        );
        // A file sent whole goes without a word.
        append(&dir, "2026-11-14.jsonl", &kept);
        reader.read_batch().unwrap();
        fs::remove_file(dir.join("2026-11-13.jsonl")).unwrap();
        assert!(reader.read_batch().unwrap().notes.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_position_in_a_file_not_made_yet_waits_for_it() {
        let dir = folder("waiting");
        let mut reader = Reader::new(&dir, "siem", at("2026-11-13.jsonl", 0));
        let batch = reader.read_batch().unwrap();
        assert_eq!((batch.records.len(), batch.lines, batch.notes.len()), (0, 0, 0));
        assert_eq!(batch.position.offset("2026-11-13.jsonl"), 0);
        let one = line(1, "now");
        append(&dir, "2026-11-13.jsonl", &one);
        assert_eq!(texts(&reader.read_batch().unwrap()), ["now"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_shorter_than_the_position_in_it_is_read_again_from_its_start() {
        let dir = folder("shorter");
        let one = line(1, "again");
        append(&dir, "2026-11-13.jsonl", &one);
        let mut reader = Reader::new(&dir, "siem", at("2026-11-13.jsonl", 100_000));
        let batch = reader.read_batch().unwrap();
        assert_eq!(texts(&batch), ["again"]);
        assert_eq!(
            batch.notes,
            [
                "siem: 2026-11-13.jsonl is shorter than the position in it; sending it again from its start"
            ]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_batch_holds_at_most_256_lines_and_the_next_goes_on() {
        let dir = folder("full");
        let text: String = (0..300).map(|seq| line(seq, "x")).collect();
        append(&dir, "2026-11-13.jsonl", &text);
        let mut reader = Reader::new(&dir, "siem", at("2026-11-13.jsonl", 0));
        let first = reader.read_batch().unwrap();
        assert_eq!(first.records.len(), 256);
        let second = reader.read_batch().unwrap();
        assert_eq!(second.records.len(), 44);
        assert_eq!(second.records[0].seq, 256);
        assert_eq!(second.position.offset("2026-11-13.jsonl"), text.len() as u64);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_batch_reads_about_64_kib_and_a_longer_line_still_goes_through() {
        let dir = folder("bytes");
        let big = line(1, &"y".repeat(100_000));
        let after = line(2, "z");
        append(&dir, "2026-11-13.jsonl", &format!("{big}{after}"));
        let mut reader = Reader::new(&dir, "siem", at("2026-11-13.jsonl", 0));
        assert_eq!(reader.read_batch().unwrap().lines, 1);
        assert_eq!(texts(&reader.read_batch().unwrap()), ["z"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_destination_starts_after_the_last_whole_line_of_each_file() {
        let dir = folder("end");
        assert_eq!(end_of_log(&dir), Position::default());
        let (one, two) = (line(1, "old"), line(2, "older"));
        append(&dir, "2026-11-12.jsonl", &one);
        append(&dir, "2026-11-13.jsonl", &format!("{one}{two}{{\"time\":\"cu"));
        append(&dir, "2026-11-14.jsonl", "");
        assert_eq!(
            end_of_log(&dir),
            at_each(&[
                ("2026-11-12.jsonl", one.len() as u64),
                ("2026-11-13.jsonl", (one.len() + two.len()) as u64),
                ("2026-11-14.jsonl", 0)
            ])
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn behind_is_what_lies_after_the_offset_in_each_file() {
        let dir = folder("behind");
        append(&dir, "2026-11-12.jsonl", &"o".repeat(1000));
        append(&dir, "2026-11-13.jsonl", &"n".repeat(500));
        append(&dir, "2026-11-14.jsonl", &"m".repeat(70));
        let position = at_each(&[("2026-11-12.jsonl", 1000), ("2026-11-13.jsonl", 200)]);
        assert_eq!(behind(&dir, &position), 300 + 70);
        assert_eq!(behind(&dir, &Position::default()), 1570);
        assert_eq!(behind(&dir, &at("2026-11-10.jsonl", 5)), 1570);
        fs::remove_dir_all(&dir).unwrap();
    }
}
