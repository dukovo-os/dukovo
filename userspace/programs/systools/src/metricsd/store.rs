// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The files metricsd keeps the machine's history in (ADR 0484): one file
//! a UTC day of one row for each step, two weeks of them, and one file a
//! UTC month of one record for each hour, a year of them, in
//! `/system/state/metrics`.
//!
//! A file is a run of records, each laid out as
//!
//! ```text
//! kind u8 | len u32 | payload, len bytes | crc32 u32 of kind, len and payload
//! ```
//!
//! with the numbers little-endian. A names record (`N`) lists the series,
//! which are the columns of the rows after it; a file starts with one and
//! another follows whenever a series is added. A row (`R`) is a time and
//! a value for each column, NaN for a missing one, and an hour (`H`) is
//! a time and the average, minimum and maximum of each column over that
//! hour. Records are only ever appended: Btrfs never rewrites an old
//! extent for it, and snapshots go on sharing them.
//!
//! [`Store`] is what metricsd writes with; [`read`] is what everything
//! that shows history reads with, the `metrics` command and the console.
//! A file that a power cut left with half a record is cut back to its last
//! whole record when metricsd starts, and a record whose checksum fails
//! ends the reading of its file. The `metricsd` unit tests cover both.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::logd;

/// The most series the history keeps.
pub const MAX_SERIES: usize = 1024;
/// The longest a series' name may be, in bytes.
pub const NAME_MAX: usize = 512;

/// The folders of the minute files and of the hour files, below the
/// store's folder.
const MINUTES_DIR: &str = "minutes";
const HOURS_DIR: &str = "hours";

/// A range that starts within this many days is read from the minute
/// files, an older one from the hour files.
pub const MINUTE_DAYS: i64 = 14;
/// Minute files older than this many days are deleted, except the
/// newest [`MINUTE_FILES`].
const MINUTE_DELETE_DAYS: i64 = 15;
const MINUTE_FILES: usize = 14;
/// Hour files older than this many months are deleted, except the newest
/// [`HOUR_FILES`]: the current month and the twelve before it.
const HOUR_DELETE_MONTHS: i64 = 13;
const HOUR_FILES: usize = 13;

/// How long rows and hours wait in memory for a disk that takes no
/// writes, in seconds; older ones are dropped.
const PENDING_SPAN: i64 = 86_400;

const DAY: i64 = 86_400;
const HOUR: i64 = 3_600;

/// The longest payload a record may have: more than the most series of
/// the longest names make, so that a damaged length is not believed.
const PAYLOAD_MAX: usize = 1 << 20;

const NAMES_KIND: u8 = b'N';
const ROW_KIND: u8 = b'R';
const HOUR_KIND: u8 = b'H';

/// A series asked for: a metric name and, optionally, one instance of it
/// (`fs.used{/data}`). Without an instance every instance of the metric
/// is meant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub metric: String,
    pub instance: Option<String>,
}

impl Selector {
    /// The selector `text` names: `cpu`, or `disk.read{vda}`.
    pub fn parse(text: &str) -> Selector {
        match split_name(text) {
            (metric, Some(instance)) => {
                Selector { metric: metric.into(), instance: Some(instance.into()) }
            }
            (metric, None) => Selector { metric: metric.into(), instance: None },
        }
    }

    /// Whether series `name` is one it asks for.
    pub fn matches(&self, name: &str) -> bool {
        let (metric, instance) = split_name(name);
        metric == self.metric
            && self.instance.as_deref().is_none_or(|wanted| instance == Some(wanted))
    }
}

/// A series name split into its metric and its instance: `net.rx{net0}`
/// is `net.rx` and `net0`.
fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.split_once('{') {
        Some((metric, rest)) => match rest.strip_suffix('}') {
            Some(instance) => (metric, Some(instance)),
            None => (name, None),
        },
        None => (name, None),
    }
}

/// One series over a range of time.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    /// `metric{instance}`, or `metric` when it has no instance.
    pub name: String,
    /// Time in seconds since 1970 and value, oldest first. A step the
    /// series has no value for has no point.
    pub points: Vec<(i64, f32)>,
}

/// What a record holds.
#[derive(Clone, Debug, PartialEq)]
enum Record {
    Names(Vec<String>),
    Row { at: i64, values: Vec<f32> },
    Hour { at: i64, values: Vec<[f32; 3]> },
}

/// The CRC-32 of IEEE 802.3 (the polynomial 0xEDB88320, as zlib and
/// Ethernet use it) over `chunks` in turn.
fn crc32(chunks: &[&[u8]]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut byte = 0;
        while byte < 256 {
            let mut crc = byte as u32;
            let mut bit = 0;
            while bit < 8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
                bit += 1;
            }
            table[byte] = crc;
            byte += 1;
        }
        table
    };
    let mut crc = !0u32;
    for &byte in chunks.iter().copied().flatten() {
        crc = TABLE[usize::from((crc as u8) ^ byte)] ^ (crc >> 8);
    }
    !crc
}

/// Appends a record of `kind` holding `payload` to `out`.
fn put_record(out: &mut Vec<u8>, kind: u8, payload: &[u8]) {
    // A payload is under PAYLOAD_MAX: at most MAX_SERIES names of at most
    // NAME_MAX bytes, or that many values.
    let mut head = [kind; 5];
    head[1..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&head);
    out.extend_from_slice(payload);
    out.extend_from_slice(&crc32(&[&head, payload]).to_le_bytes());
}

fn put_names(out: &mut Vec<u8>, names: &[String]) {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(names.len() as u16).to_le_bytes());
    for name in names {
        payload.extend_from_slice(&(name.len() as u16).to_le_bytes());
        payload.extend_from_slice(name.as_bytes());
    }
    put_record(out, NAMES_KIND, &payload);
}

fn put_row(out: &mut Vec<u8>, at: i64, values: &[f32]) {
    let mut payload = Vec::with_capacity(10 + 4 * values.len());
    payload.extend_from_slice(&at.to_le_bytes());
    payload.extend_from_slice(&(values.len() as u16).to_le_bytes());
    for value in values {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    put_record(out, ROW_KIND, &payload);
}

fn put_hour(out: &mut Vec<u8>, at: i64, values: &[[f32; 3]]) {
    let mut payload = Vec::with_capacity(10 + 12 * values.len());
    payload.extend_from_slice(&at.to_le_bytes());
    payload.extend_from_slice(&(values.len() as u16).to_le_bytes());
    for value in values.iter().flatten() {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    put_record(out, HOUR_KIND, &payload);
}

/// A cursor over a record's payload.
struct Payload<'a>(&'a [u8]);

impl<'a> Payload<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let (taken, rest) = self.0.split_at_checked(len)?;
        self.0 = rest;
        Some(taken)
    }

    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    /// Whether the payload has been read to its end.
    fn done(&self) -> bool {
        self.0.is_empty()
    }
}

fn decode(kind: u8, payload: &[u8]) -> Option<Record> {
    let mut payload = Payload(payload);
    let record = match kind {
        NAMES_KIND => {
            let count = payload.u16()?;
            let mut names = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                let len = usize::from(payload.u16()?);
                names.push(String::from_utf8(payload.take(len)?.to_vec()).ok()?);
            }
            Record::Names(names)
        }
        ROW_KIND => {
            let at = payload.i64()?;
            let count = payload.u16()?;
            let values = (0..count).map(|_| payload.f32()).collect::<Option<Vec<f32>>>()?;
            Record::Row { at, values }
        }
        HOUR_KIND => {
            let at = payload.i64()?;
            let count = payload.u16()?;
            let mut values = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                values.push([payload.f32()?, payload.f32()?, payload.f32()?]);
            }
            Record::Hour { at, values }
        }
        _ => return None,
    };
    payload.done().then_some(record)
}

/// The records of a file's bytes, up to the first one that is torn, has
/// a wrong checksum or is not one of ours.
struct Records<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Records<'a> {
    fn new(bytes: &'a [u8]) -> Records<'a> {
        Records { bytes, at: 0 }
    }

    /// Where the records read so far end: the length a file is cut to,
    /// once all have been read.
    fn length(&self) -> usize {
        self.at
    }
}

impl Iterator for Records<'_> {
    type Item = Record;

    fn next(&mut self) -> Option<Record> {
        let rest = &self.bytes[self.at..];
        let head = rest.get(..5)?;
        let len = u32::from_le_bytes(head[1..].try_into().ok()?) as usize;
        if len > PAYLOAD_MAX {
            return None;
        }
        let payload = rest.get(5..5 + len)?;
        let stored = u32::from_le_bytes(rest.get(5 + len..9 + len)?.try_into().ok()?);
        if stored != crc32(&[head, payload]) {
            return None;
        }
        let record = decode(head[0], payload)?;
        self.at += 9 + len;
        Some(record)
    }
}

/// The name of day `days` since 1970's file: `2026-11-13`.
fn day_name(days: i64) -> String {
    let (year, month, day) = logd::civil(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The day a minute file's name stands for, as days since 1970.
fn parse_day(name: &str) -> Option<i64> {
    let mut parts = name.splitn(3, '-');
    let year: i64 = parts.next().filter(|p| p.len() == 4)?.parse().ok()?;
    let month: u32 = parts.next().filter(|p| p.len() == 2)?.parse().ok()?;
    let day: u32 = parts.next().filter(|p| p.len() == 2)?.parse().ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then(|| logd::days(year, month, day))
}

/// A month counted from year 0: the number the hour files are ordered by.
fn month_number(days: i64) -> i64 {
    let (year, month, _) = logd::civil(days);
    year * 12 + i64::from(month) - 1
}

/// The name of the hour file of month `number`: `2026-11`.
fn month_name(number: i64) -> String {
    format!("{:04}-{:02}", number.div_euclid(12), number.rem_euclid(12) + 1)
}

/// The month an hour file's name stands for, as [`month_number`] counts.
fn parse_month(name: &str) -> Option<i64> {
    let (year, month) = name.split_once('-')?;
    let year: i64 = year.parse().ok().filter(|_| year.len() == 4)?;
    let month: i64 = month.parse().ok().filter(|m| month.len() == 2 && (1..=12).contains(m))?;
    Some(year * 12 + month - 1)
}

/// The first day of month `number`, as days since 1970.
fn month_start(number: i64) -> i64 {
    let month = u32::try_from(number.rem_euclid(12) + 1).unwrap_or(1);
    logd::days(number.div_euclid(12), month, 1)
}

/// The sums of an hour's values for each series, from which its record
/// is made.
#[derive(Default)]
struct HourSums {
    /// The hour, as seconds since 1970 divided by 3600.
    hour: i64,
    count: Vec<u32>,
    sum: Vec<f64>,
    min: Vec<f32>,
    max: Vec<f32>,
}

impl HourSums {
    fn starting(hour: i64) -> HourSums {
        HourSums { hour, ..HourSums::default() }
    }

    /// Takes in a row's values, the missing ones left out.
    fn add(&mut self, values: &[f32]) {
        if values.len() > self.count.len() {
            self.count.resize(values.len(), 0);
            self.sum.resize(values.len(), 0.0);
            self.min.resize(values.len(), f32::NAN);
            self.max.resize(values.len(), f32::NAN);
        }
        for (column, &value) in values.iter().enumerate() {
            if value.is_nan() {
                continue;
            }
            self.count[column] += 1;
            self.sum[column] += f64::from(value);
            self.min[column] =
                if self.min[column].is_nan() { value } else { self.min[column].min(value) };
            self.max[column] =
                if self.max[column].is_nan() { value } else { self.max[column].max(value) };
        }
    }

    /// The average, minimum and maximum of each series; NaN where the hour
    /// had no value for it.
    fn finish(&self) -> Vec<[f32; 3]> {
        (0..self.count.len())
            .map(|column| match self.count[column] {
                0 => [f32::NAN; 3],
                count => [
                    (self.sum[column] / f64::from(count)) as f32,
                    self.min[column],
                    self.max[column],
                ],
            })
            .collect()
    }

    fn is_empty(&self) -> bool {
        self.count.iter().all(|&count| count == 0)
    }
}

/// Rows as a file holds them: the time and a value for each column.
type Rows = Vec<(i64, Vec<f32>)>;

/// A file being appended to: how far it is known to be whole, and the
/// names its last names record gave.
struct Tail {
    path: PathBuf,
    good: u64,
    names: Vec<String>,
}

/// The store metricsd writes: rows kept in memory until [`Store::flush`]
/// appends them to the day's file, and the hours they sum to.
pub struct Store {
    dir: PathBuf,
    names: Vec<String>,
    index: HashMap<String, usize>,
    /// Rows not yet written, each as long as the names were when it was
    /// taken.
    pending: Vec<(i64, Vec<f32>)>,
    /// The hour now being summed, and the hours complete and not yet
    /// written.
    sums: HourSums,
    finished: Vec<(i64, Vec<[f32; 3]>)>,
    minute_file: Option<Tail>,
    hour_file: Option<Tail>,
    /// The time of the newest row, which says what is old enough to
    /// delete.
    newest: i64,
    /// The first series that was left out, until it is taken.
    left_out: Option<String>,
}

impl Store {
    /// Opens the store in `dir` at time `now`, seconds since 1970: makes
    /// its folders, reads the day's file, cuts it back to its last whole
    /// record, and carries on with its names and the earlier rows of the
    /// hour.
    ///
    /// # Errors
    ///
    /// A folder that cannot be made or a file that cannot be read or cut.
    pub fn open(dir: &Path, now: i64) -> io::Result<Store> {
        fs::create_dir_all(dir.join(MINUTES_DIR))?;
        fs::create_dir_all(dir.join(HOURS_DIR))?;
        let mut store = Store {
            dir: dir.to_path_buf(),
            names: Vec::new(),
            index: HashMap::new(),
            pending: Vec::new(),
            sums: HourSums::starting(now.div_euclid(HOUR)),
            finished: Vec::new(),
            minute_file: None,
            hour_file: None,
            newest: now,
            left_out: None,
        };
        let (tail, rows) = load_tail(&minute_path(dir, now.div_euclid(DAY)))?;
        store.names.clone_from(&tail.names);
        store.index = tail.names.iter().cloned().zip(0..).collect();
        if let Some(&(at, _)) = rows.last() {
            store.sums = HourSums::starting(at.div_euclid(HOUR));
            for (_, values) in &rows {
                store.sums.add(values);
            }
        }
        store.minute_file = Some(tail);
        keep(dir, now.div_euclid(DAY));
        Ok(store)
    }

    /// The series the store has columns for, in order.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Takes in the row of time `at`, seconds since 1970: a value for each
    /// series it has. A series that is new gets a column; one past
    /// [`MAX_SERIES`], or with a name over [`NAME_MAX`] bytes, is left out.
    pub fn push(&mut self, at: i64, row: &[(String, f32)]) {
        let mut values = vec![f32::NAN; self.names.len()];
        for (name, value) in row {
            let Some(column) = self.column(name) else { continue };
            if column >= values.len() {
                values.resize(column + 1, f32::NAN);
            }
            values[column] = *value;
        }
        self.sum_hour(at, &values);
        self.pending.push((at, values));
        self.newest = self.newest.max(at);
        self.drop_old(at);
    }

    /// The column of series `name`, made if it is new, unless it may not
    /// be.
    fn column(&mut self, name: &str) -> Option<usize> {
        if let Some(&column) = self.index.get(name) {
            return Some(column);
        }
        if self.names.len() >= MAX_SERIES || name.len() > NAME_MAX {
            self.left_out.get_or_insert_with(|| name.to_string());
            return None;
        }
        self.names.push(name.to_string());
        self.index.insert(name.to_string(), self.names.len() - 1);
        Some(self.names.len() - 1)
    }

    /// Adds a row to the hour being summed, and sets the hour aside when
    /// the row is of a later one.
    fn sum_hour(&mut self, at: i64, values: &[f32]) {
        let hour = at.div_euclid(HOUR);
        if hour != self.sums.hour {
            let done = std::mem::replace(&mut self.sums, HourSums::starting(hour));
            if !done.is_empty() {
                self.finished.push((done.hour * HOUR, done.finish()));
            }
        }
        self.sums.add(values);
    }

    /// Lets go of what has waited longer than [`PENDING_SPAN`] for a disk.
    fn drop_old(&mut self, now: i64) {
        let oldest = now - PENDING_SPAN;
        self.pending.retain(|&(at, _)| at >= oldest);
        self.finished.retain(|&(at, _)| at >= oldest);
    }

    /// The name of the first series that was left out, once: the caller
    /// logs it.
    pub fn take_left_out(&mut self) -> Option<String> {
        self.left_out.take()
    }

    /// Appends the rows waiting to the files of their days and the hours
    /// complete to the files of their months, each in one write that is
    /// then synced; what is written is forgotten, and what is not stays
    /// for the next call.
    ///
    /// # Errors
    ///
    /// The first write that failed. A file that a failed write left with
    /// half a record is cut back before the next write to it.
    pub fn flush(&mut self) -> io::Result<()> {
        let hours = !self.finished.is_empty();
        self.flush_rows()?;
        self.flush_hours()?;
        if hours {
            keep(&self.dir, self.newest.div_euclid(DAY));
        }
        Ok(())
    }

    fn flush_rows(&mut self) -> io::Result<()> {
        while let Some(&(first, _)) = self.pending.first() {
            let day = first.div_euclid(DAY);
            let count = self.pending.iter().take_while(|(at, _)| at.div_euclid(DAY) == day).count();
            let path = minute_path(&self.dir, day);
            let mut tail = take_tail(&mut self.minute_file, &path)?;
            let mut buffer = names_record(&tail, &self.names);
            for (at, values) in &self.pending[..count] {
                put_row(&mut buffer, *at, values);
            }
            let written = append(&mut tail, &self.names, &buffer);
            self.minute_file = Some(tail);
            written?;
            self.pending.drain(..count);
        }
        Ok(())
    }

    fn flush_hours(&mut self) -> io::Result<()> {
        while let Some(&(first, _)) = self.finished.first() {
            let month = month_number(first.div_euclid(DAY));
            let count = self
                .finished
                .iter()
                .take_while(|(at, _)| month_number(at.div_euclid(DAY)) == month)
                .count();
            let path = self.dir.join(HOURS_DIR).join(month_name(month));
            let mut tail = take_tail(&mut self.hour_file, &path)?;
            let mut buffer = names_record(&tail, &self.names);
            for (at, values) in &self.finished[..count] {
                put_hour(&mut buffer, *at, values);
            }
            let written = append(&mut tail, &self.names, &buffer);
            self.hour_file = Some(tail);
            written?;
            self.finished.drain(..count);
        }
        Ok(())
    }
}

fn minute_path(dir: &Path, day: i64) -> PathBuf {
    dir.join(MINUTES_DIR).join(day_name(day))
}

/// The file `tail` holds, if it is `path`'s, else `path`'s as it is on
/// the disk.
fn take_tail(held: &mut Option<Tail>, path: &Path) -> io::Result<Tail> {
    match held.take() {
        Some(tail) if tail.path == path => Ok(tail),
        _ => Ok(load_tail(path)?.0),
    }
}

/// The names record that a file whose last one is in `tail` needs before
/// more rows, if it needs one.
fn names_record(tail: &Tail, names: &[String]) -> Vec<u8> {
    let mut buffer = Vec::new();
    if tail.names != names {
        put_names(&mut buffer, names);
    }
    buffer
}

/// Reads the file at `path`, which may not be there, cuts it back to its
/// last whole record, and gives its end and last names, with the rows of
/// the last hour in it.
fn load_tail(path: &Path) -> io::Result<(Tail, Rows)> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(err) => return Err(err),
    };
    let mut records = Records::new(&bytes);
    let mut names = Vec::new();
    let mut rows = Rows::new();
    for record in records.by_ref() {
        match record {
            Record::Names(found) => names = found,
            Record::Row { at, values } => {
                if rows
                    .last()
                    .is_some_and(|&(last, _)| last.div_euclid(HOUR) != at.div_euclid(HOUR))
                {
                    rows.clear();
                }
                rows.push((at, values));
            }
            Record::Hour { .. } => {}
        }
    }
    let good = records.length();
    if good < bytes.len() {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(good as u64)?;
        file.sync_all()?;
    }
    Ok((Tail { path: path.to_path_buf(), good: good as u64, names }, rows))
}

/// Appends `buffer` to the file of `tail` in one write and syncs it. A file
/// that a failed write left longer than `tail.good` is cut back first, so
/// that no half record sits before what is written now.
fn append(tail: &mut Tail, names: &[String], buffer: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(&tail.path)?;
    if file.metadata()?.len() != tail.good {
        file.set_len(tail.good)?;
    }
    file.write_all(buffer)?;
    file.sync_data()?;
    if tail.good == 0 {
        // The new file's name has to survive a power cut too.
        if let Some(folder) = tail.path.parent() {
            File::open(folder)?.sync_all()?;
        }
    }
    tail.good += buffer.len() as u64;
    tail.names = names.to_vec();
    Ok(())
}

/// Deletes the files too old to keep, `today` being a day since 1970. The
/// newest of each kind stay, however old, so that a machine that was off
/// for a month still has its last weeks.
fn keep(dir: &Path, today: i64) {
    let this_month = month_number(today);
    prune(&dir.join(MINUTES_DIR), MINUTE_FILES, |name| {
        parse_day(name).map(|day| day < today - MINUTE_DELETE_DAYS)
    });
    prune(&dir.join(HOURS_DIR), HOUR_FILES, |name| {
        parse_month(name).map(|month| month < this_month - HOUR_DELETE_MONTHS)
    });
}

/// Deletes the files of `dir` that `expired` says are, but not the
/// `newest` of the ones it knows by name. `expired` is `None` for a file
/// that is not the store's, which is left alone.
fn prune(dir: &Path, newest: usize, expired: impl Fn(&str) -> Option<bool>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| expired(name).is_some())
        .collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    for name in names.into_iter().skip(newest).filter(|name| expired(name) == Some(true)) {
        // A file that cannot be removed now is tried again after the next
        // hour.
        let _ = fs::remove_file(dir.join(name));
    }
}

/// The history of the series `select` names from `from` up to `to`,
/// seconds since 1970, from the store in `dir`; every series when `select`
/// is empty.
///
/// A range that starts within the last [`MINUTE_DAYS`] days is read from
/// the minute files, an older one from the hour files, which give their
/// averages. With `every`, the points are averaged into steps of that many
/// seconds, each at its step's start. A file that is not there is no data,
/// and one with a record gone bad gives what is before it.
///
/// # Errors
///
/// A file or folder that exists and cannot be read.
pub fn read(
    dir: &Path,
    from: i64,
    to: i64,
    select: &[Selector],
    every: Option<u32>,
) -> io::Result<Vec<Series>> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    read_at(dir, now, from, to, select, every)
}

/// [`read`] as it is at time `now`.
fn read_at(
    dir: &Path,
    now: i64,
    from: i64,
    to: i64,
    select: &[Selector],
    every: Option<u32>,
) -> io::Result<Vec<Series>> {
    let minutes = from >= now - MINUTE_DAYS * DAY;
    let mut found: BTreeMap<String, Vec<(i64, f32)>> = BTreeMap::new();
    for (path, start, end) in data_files(dir, minutes)? {
        if end > from && start < to {
            read_file(&path, minutes, (from, to), select, &mut found)?;
        }
    }
    Ok(found
        .into_iter()
        .map(|(name, mut points)| {
            points.sort_by_key(|&(at, _)| at);
            let points = match every {
                Some(step) => average(&points, i64::from(step)),
                None => points,
            };
            Series { name, points }
        })
        .filter(|series| !series.points.is_empty())
        .collect())
}

/// The minute files, or the hour files, in the order of their days, with
/// the seconds each one's range starts and ends at.
fn data_files(dir: &Path, minutes: bool) -> io::Result<Vec<(PathBuf, i64, i64)>> {
    let folder = dir.join(if minutes { MINUTES_DIR } else { HOURS_DIR });
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        let days = if minutes {
            parse_day(&name).map(|day| (day, day + 1))
        } else {
            parse_month(&name).map(|month| (month_start(month), month_start(month + 1)))
        };
        if let Some((first, after)) = days {
            files.push((entry.path(), first * DAY, after * DAY));
        }
    }
    files.sort_by_key(|&(_, start, _)| start);
    Ok(files)
}

/// Adds the points of `range` of the file at `path` for the series
/// `select` names to `found`.
fn read_file(
    path: &Path,
    minutes: bool,
    range: (i64, i64),
    select: &[Selector],
    found: &mut BTreeMap<String, Vec<(i64, f32)>>,
) -> io::Result<()> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    // The name of each column, for those that are asked for.
    let mut columns: Vec<Option<String>> = Vec::new();
    let mut add = |columns: &[Option<String>], at: i64, values: &mut dyn Iterator<Item = f32>| {
        if at < range.0 || at >= range.1 {
            return;
        }
        for (name, value) in columns.iter().zip(values) {
            let Some(name) = name else { continue };
            if value.is_nan() {
                continue;
            }
            match found.get_mut(name) {
                Some(points) => points.push((at, value)),
                None => {
                    found.insert(name.clone(), vec![(at, value)]);
                }
            }
        }
    };
    for record in Records::new(&bytes) {
        match record {
            Record::Names(names) => {
                columns = names
                    .into_iter()
                    .map(|name| {
                        (select.is_empty() || select.iter().any(|s| s.matches(&name)))
                            .then_some(name)
                    })
                    .collect();
            }
            Record::Row { at, values } if minutes => add(&columns, at, &mut values.into_iter()),
            Record::Hour { at, values } if !minutes => {
                add(&columns, at, &mut values.into_iter().map(|[average, _, _]| average));
            }
            _ => {}
        }
    }
    Ok(())
}

/// `points`, oldest first, averaged into steps of `step` seconds, each at
/// the step's start.
fn average(points: &[(i64, f32)], step: i64) -> Vec<(i64, f32)> {
    let step = step.max(1);
    let mut out: Vec<(i64, f32)> = Vec::new();
    let mut sum = 0.0f64;
    let mut count = 0u32;
    let mut start = 0;
    for &(at, value) in points {
        let bucket = at.div_euclid(step) * step;
        if count > 0 && bucket != start {
            out.push((start, (sum / f64::from(count)) as f32));
            sum = 0.0;
            count = 0;
        }
        start = bucket;
        sum += f64::from(value);
        count += 1;
    }
    if count > 0 {
        out.push((start, (sum / f64::from(count)) as f32));
    }
    out
}

/// The series the history holds, those of the newest minute file's last
/// names record; none while there is no history.
///
/// # Errors
///
/// A file or folder that exists and cannot be read.
pub fn series(dir: &Path) -> io::Result<Vec<String>> {
    let Some((path, _, _)) = data_files(dir, true)?.pop() else { return Ok(Vec::new()) };
    let bytes = fs::read(path)?;
    let names = Records::new(&bytes).fold(Vec::new(), |names, record| match record {
        Record::Names(found) => found,
        _ => names,
    });
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-13 10:00:00 UTC.
    const TEN: i64 = 1_794_564_000;

    /// A folder of the test's own.
    fn folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("metricsd-store-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn row(values: &[(&str, f32)]) -> Vec<(String, f32)> {
        values.iter().map(|(name, value)| ((*name).to_string(), *value)).collect()
    }

    fn all(dir: &Path, now: i64, from: i64, to: i64) -> Vec<Series> {
        read_at(dir, now, from, to, &[], None).unwrap()
    }

    fn points(series: &[Series], name: &str) -> Vec<(i64, f32)> {
        series.iter().find(|s| s.name == name).map_or_default(|s| s.points.clone())
    }

    #[test]
    fn the_checksum_is_the_one_zlib_has() {
        assert_eq!(crc32(&[b"123456789"]), 0xCBF4_3926);
        assert_eq!(crc32(&[b"1234", b"56789"]), 0xCBF4_3926);
        assert_eq!(crc32(&[]), 0);
    }

    #[test]
    fn day_and_month_files_are_named_by_their_dates() {
        assert_eq!(day_name(TEN.div_euclid(DAY)), "2026-11-13");
        assert_eq!(parse_day("2026-11-13"), Some(TEN.div_euclid(DAY)));
        for bad in ["2026-11", "2026-13-01", "26-11-13", "2026-11-13.old", "latest"] {
            assert_eq!(parse_day(bad), None, "{bad}");
        }
        let november = month_number(TEN.div_euclid(DAY));
        assert_eq!(month_name(november), "2026-11");
        assert_eq!(parse_month("2026-11"), Some(november));
        assert_eq!(month_name(november + 2), "2027-01");
        assert_eq!(month_start(november) * DAY, TEN - 12 * DAY - 10 * HOUR);
        assert_eq!(parse_month("2026-00"), None);
    }

    #[test]
    fn names_and_rows_read_back() {
        let dir = folder("rows");
        let mut store = Store::open(&dir, TEN).unwrap();
        store.push(TEN, &row(&[("cpu", 1.5), ("fs.used{/}", 100.0)]));
        store.push(TEN + 60, &row(&[("cpu", 2.5)]));
        store.push(TEN + 120, &row(&[("cpu", f32::NAN), ("fs.used{/}", 300.0)]));
        store.flush().unwrap();

        let found = all(&dir, TEN + 200, TEN, TEN + 200);
        assert_eq!(points(&found, "cpu"), [(TEN, 1.5), (TEN + 60, 2.5)]);
        assert_eq!(points(&found, "fs.used{/}"), [(TEN, 100.0), (TEN + 120, 300.0)]);
        // The range is from, up to but not including to.
        let found = all(&dir, TEN + 200, TEN + 60, TEN + 120);
        assert_eq!(points(&found, "cpu"), [(TEN + 60, 2.5)]);
        assert!(points(&found, "fs.used{/}").is_empty());
        assert_eq!(series(&dir).unwrap(), ["cpu", "fs.used{/}"]);
    }

    #[test]
    fn a_torn_last_record_is_cut_at_start() {
        let dir = folder("torn");
        let mut store = Store::open(&dir, TEN).unwrap();
        store.push(TEN, &row(&[("cpu", 1.0)]));
        store.push(TEN + 60, &row(&[("cpu", 2.0)]));
        store.flush().unwrap();
        let path = minute_path(&dir, TEN.div_euclid(DAY));
        let whole = fs::metadata(&path).unwrap().len();
        // A power cut in the middle of the next row's write.
        let mut half = Vec::new();
        put_row(&mut half, TEN + 120, &[3.0]);
        half.truncate(half.len() - 5);
        OpenOptions::new().append(true).open(&path).unwrap().write_all(&half).unwrap();
        assert!(fs::metadata(&path).unwrap().len() > whole);

        let mut store = Store::open(&dir, TEN + 130).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), whole);
        assert_eq!(store.names(), ["cpu"]);
        store.push(TEN + 180, &row(&[("cpu", 4.0)]));
        store.flush().unwrap();
        let found = all(&dir, TEN + 200, TEN, TEN + 200);
        assert_eq!(points(&found, "cpu"), [(TEN, 1.0), (TEN + 60, 2.0), (TEN + 180, 4.0)]);
    }

    #[test]
    fn a_series_added_in_the_middle_gets_a_column_and_a_names_record() {
        let dir = folder("new-series");
        let mut store = Store::open(&dir, TEN).unwrap();
        store.push(TEN, &row(&[("cpu", 1.0)]));
        store.flush().unwrap();
        store.push(TEN + 60, &row(&[("cpu", 2.0), ("load", 0.5)]));
        store.push(TEN + 120, &row(&[("load", 0.7)]));
        store.flush().unwrap();

        let bytes = fs::read(minute_path(&dir, TEN.div_euclid(DAY))).unwrap();
        let kinds: Vec<&str> = Records::new(&bytes)
            .map(|record| match record {
                Record::Names(_) => "N",
                Record::Row { .. } => "R",
                Record::Hour { .. } => "H",
            })
            .collect();
        assert_eq!(kinds, ["N", "R", "N", "R", "R"]);
        let found = all(&dir, TEN + 200, TEN, TEN + 200);
        assert_eq!(points(&found, "cpu"), [(TEN, 1.0), (TEN + 60, 2.0)]);
        assert_eq!(points(&found, "load"), [(TEN + 60, 0.5), (TEN + 120, 0.7)]);
    }

    #[test]
    fn a_series_past_the_most_is_left_out_and_said_once() {
        let dir = folder("cap");
        let mut store = Store::open(&dir, TEN).unwrap();
        let many: Vec<(String, f32)> = (0..=MAX_SERIES).map(|n| (format!("s{n}"), 1.0)).collect();
        store.push(TEN, &many);
        store.push(TEN + 60, &row(&[("another", 1.0)]));
        assert_eq!(store.names().len(), MAX_SERIES);
        assert_eq!(store.take_left_out().as_deref(), Some("s1024"));
        assert_eq!(store.take_left_out(), None);
        store.push(TEN + 120, &row(&[(&"n".repeat(NAME_MAX + 1), 1.0)]));
        assert_eq!(store.take_left_out().map(|name| name.len()), Some(NAME_MAX + 1));
    }

    #[test]
    fn an_hour_is_folded_when_a_row_of_the_next_comes() {
        let dir = folder("hours");
        let mut store = Store::open(&dir, TEN).unwrap();
        store.push(TEN + 600, &row(&[("cpu", 1.0), ("load", 5.0)]));
        store.push(TEN + 1200, &row(&[("cpu", 2.0), ("load", f32::NAN)]));
        store.push(TEN + 1800, &row(&[("cpu", 6.0)]));
        store.push(TEN + HOUR, &row(&[("cpu", 100.0)]));
        store.flush().unwrap();

        let month = month_name(month_number(TEN.div_euclid(DAY)));
        let bytes = fs::read(dir.join(HOURS_DIR).join(month)).unwrap();
        let hours: Vec<Record> = Records::new(&bytes).collect();
        assert_eq!(
            hours,
            [
                Record::Names(vec!["cpu".into(), "load".into()]),
                Record::Hour { at: TEN, values: vec![[3.0, 1.0, 6.0], [5.0, 5.0, 5.0]] },
            ]
        );
    }

    #[test]
    fn an_hour_after_a_restart_in_the_middle_includes_its_earlier_rows() {
        let dir = folder("restart");
        let mut store = Store::open(&dir, TEN).unwrap();
        store.push(TEN + 600, &row(&[("cpu", 1.0)]));
        store.push(TEN + 1200, &row(&[("cpu", 3.0)]));
        store.flush().unwrap();
        drop(store);

        let mut store = Store::open(&dir, TEN + 1500).unwrap();
        store.push(TEN + 1800, &row(&[("cpu", 8.0)]));
        store.push(TEN + HOUR + 60, &row(&[("cpu", 50.0)]));
        store.flush().unwrap();

        let series = read_at(&dir, TEN + 3 * HOUR, 0, TEN + 3 * HOUR, &[], None).unwrap();
        // The hour files are read for a range that starts long ago.
        assert_eq!(points(&series, "cpu"), [(TEN, 4.0)]);
    }

    #[test]
    fn a_range_that_starts_long_ago_is_read_from_the_hours() {
        let dir = folder("long");
        let mut store = Store::open(&dir, TEN).unwrap();
        for step in 0..120 {
            store.push(TEN + step * 60, &row(&[("cpu", step as f32)]));
        }
        store.flush().unwrap();
        let later = TEN + 40 * DAY;
        let found = read_at(&dir, later, TEN, TEN + 3 * HOUR, &[], None).unwrap();
        // Ten o'clock's average of 0..59, and no eleven o'clock yet: its
        // hour is complete only when a row of the next one comes.
        assert_eq!(points(&found, "cpu"), [(TEN, 29.5)]);
        let found = read_at(&dir, TEN + 3 * HOUR, TEN, TEN + 3 * HOUR, &[], None).unwrap();
        assert_eq!(found[0].points.len(), 120);
    }

    #[test]
    fn rows_are_read_across_days_and_averaged_into_wider_steps() {
        let dir = folder("days");
        let midnight = TEN - 10 * HOUR;
        let mut store = Store::open(&dir, midnight - 120).unwrap();
        for step in 0..4 {
            store.push(midnight - 120 + step * 60, &row(&[("cpu", step as f32), ("load", 1.0)]));
        }
        store.flush().unwrap();
        assert!(minute_path(&dir, midnight.div_euclid(DAY) - 1).exists());
        assert!(minute_path(&dir, midnight.div_euclid(DAY)).exists());

        let now = midnight + HOUR;
        let found =
            read_at(&dir, now, midnight - 300, now, &[Selector::parse("cpu")], None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].points,
            [(midnight - 120, 0.0), (midnight - 60, 1.0), (midnight, 2.0), (midnight + 60, 3.0)]
        );
        let found =
            read_at(&dir, now, midnight - 300, now, &[Selector::parse("cpu")], Some(120)).unwrap();
        assert_eq!(found[0].points, [(midnight - 120, 0.5), (midnight, 2.5)]);
    }

    #[test]
    fn selectors_name_a_metric_and_maybe_an_instance() {
        let one = Selector::parse("fs.used{/data}");
        let any = Selector::parse("fs.used");
        assert_eq!(one, Selector { metric: "fs.used".into(), instance: Some("/data".into()) });
        assert!(one.matches("fs.used{/data}") && !one.matches("fs.used{/}"));
        assert!(
            any.matches("fs.used{/data}") && any.matches("fs.used{/}") && any.matches("fs.used")
        );
        assert!(!any.matches("fs.size{/}") && !any.matches("fs.used.more"));
        assert!(
            Selector::parse("cpu").matches("cpu") && !Selector::parse("cpu").matches("cpu.user")
        );
        // A name with an unfinished brace is a metric of its own.
        assert_eq!(split_name("odd{name"), ("odd{name", None));
    }

    #[test]
    fn a_bad_checksum_ends_the_reading_of_its_file() {
        let dir = folder("crc");
        let mut store = Store::open(&dir, TEN).unwrap();
        for step in 0..4 {
            store.push(TEN + step * 60, &row(&[("cpu", step as f32)]));
        }
        store.flush().unwrap();
        let path = minute_path(&dir, TEN.div_euclid(DAY));
        let mut bytes = fs::read(&path).unwrap();
        // A byte of the second row's payload: the names record and the
        // first row come before it.
        let (mut names, mut first) = (Vec::new(), Vec::new());
        put_names(&mut names, &["cpu".to_string()]);
        put_row(&mut first, TEN, &[0.0]);
        bytes[names.len() + first.len() + 6] ^= 0xff;
        fs::write(&path, &bytes).unwrap();

        let found = all(&dir, TEN + 300, TEN, TEN + 300);
        assert_eq!(points(&found, "cpu"), [(TEN, 0.0)]);
    }

    #[test]
    fn files_too_old_are_deleted_but_the_newest_stay() {
        let dir = folder("keep");
        let today = TEN.div_euclid(DAY);
        let this_month = month_number(today);
        fs::create_dir_all(dir.join(MINUTES_DIR)).unwrap();
        fs::create_dir_all(dir.join(HOURS_DIR)).unwrap();
        for ago in (0..=20).chain([60]) {
            fs::write(minute_path(&dir, today - ago), b"").unwrap();
            fs::write(dir.join(HOURS_DIR).join(month_name(this_month - ago)), b"").unwrap();
        }
        fs::write(dir.join(MINUTES_DIR).join("notes"), b"left alone").unwrap();
        let left = |folder: &str| {
            let mut names: Vec<String> = fs::read_dir(dir.join(folder))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        };

        keep(&dir, today);
        // Older than 15 days goes; 15 days itself stays, and so does a
        // file that is not the store's.
        let mut days: Vec<String> = (0..=15).map(|ago| day_name(today - ago)).collect();
        days.push("notes".into());
        days.sort();
        assert_eq!(left(MINUTES_DIR), days);
        let mut months: Vec<String> = (0..=13).map(|ago| month_name(this_month - ago)).collect();
        months.sort();
        assert_eq!(left(HOURS_DIR), months);

        // A machine that was off for over a year keeps its newest files,
        // the 14 days and the current month and 12 before it.
        keep(&dir, today + 400);
        assert_eq!(left(MINUTES_DIR).len(), MINUTE_FILES + 1);
        assert_eq!(left(HOURS_DIR).len(), HOUR_FILES);
        assert!(left(MINUTES_DIR).contains(&day_name(today - 13)));
    }

    #[test]
    fn a_failed_write_keeps_the_rows_for_the_next_try() {
        let dir = folder("full");
        let mut store = Store::open(&dir, TEN).unwrap();
        store.push(TEN, &row(&[("cpu", 1.0)]));
        store.flush().unwrap();
        // The day's file is a folder now, which no write can append to.
        let path = minute_path(&dir, TEN.div_euclid(DAY));
        let kept = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        store.push(TEN + 60, &row(&[("cpu", 2.0)]));
        store.push(TEN + 120, &row(&[("cpu", 3.0)]));
        assert!(store.flush().is_err());
        assert!(store.flush().is_err());
        fs::remove_dir(&path).unwrap();
        fs::write(&path, &kept).unwrap();
        store.flush().unwrap();
        let found = all(&dir, TEN + 300, TEN, TEN + 300);
        assert_eq!(points(&found, "cpu"), [(TEN, 1.0), (TEN + 60, 2.0), (TEN + 120, 3.0)]);
    }

    #[test]
    fn half_a_record_from_a_failed_write_is_cut_before_the_next() {
        let dir = folder("garbage");
        let mut store = Store::open(&dir, TEN).unwrap();
        store.push(TEN, &row(&[("cpu", 1.0)]));
        store.flush().unwrap();
        // What a write that ran out of room leaves behind.
        let path = minute_path(&dir, TEN.div_euclid(DAY));
        OpenOptions::new().append(true).open(&path).unwrap().write_all(&[b'R', 9, 0, 0]).unwrap();
        store.push(TEN + 60, &row(&[("cpu", 2.0)]));
        store.flush().unwrap();
        let found = all(&dir, TEN + 300, TEN, TEN + 300);
        assert_eq!(points(&found, "cpu"), [(TEN, 1.0), (TEN + 60, 2.0)]);
    }

    #[test]
    fn rows_that_waited_over_a_day_for_the_disk_are_dropped() {
        let dir = folder("old");
        let mut store = Store::open(&dir, TEN).unwrap();
        store.push(TEN, &row(&[("cpu", 1.0)]));
        store.push(TEN + PENDING_SPAN + 60, &row(&[("cpu", 2.0)]));
        assert_eq!(store.pending.len(), 1);
    }
}
