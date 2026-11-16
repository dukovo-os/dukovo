// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! QEMU's write log, and the disks a power cut could leave (ADR 0536).
//!
//! QEMU's `blklogwrites` block driver records every write, zero write,
//! discard and flush that reaches a disk, in the format of Linux's
//! dm-log-writes: a superblock in sector 0, then for each operation one
//! sector that describes it, followed by the sectors it wrote. QEMU
//! writes an entry once the disk write has completed, so the log is in
//! completion order, and each write the guest saw complete before it
//! asked for a flush comes before that flush's entry.
//!
//! A [`Log`] reads that file. [`cuts`] names the states a power cut can
//! leave the disk in, and a [`Disk`] rebuilds one by writing the logged
//! data onto a copy of the image the recording started from. `ob test
//! replay` boots each rebuilt disk; when a cut fails, `ob test replay
//! --log` and `--point` boot that cut again from the kept log.

use std::fs::File;
use std::ops::Range;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::process;
use crate::{Context as _, Error, Result};

/// The log superblock's magic number, as dm-log-writes has it.
const MAGIC: u64 = 0x6a_7366_7773_6872;

/// The only version of the log format.
const VERSION: u64 = 1;

/// The superblock's length: magic, version, entry count and sector size.
const SUPERBLOCK: usize = 28;

/// An entry's description: sector, sector count, flags and data length.
const ENTRY_HEAD: usize = 32;

/// The flags of a log entry.
const FLAG_FLUSH: u64 = 1;
const FLAG_FUA: u64 = 2;
const FLAG_DISCARD: u64 = 4;
const FLAG_MARK: u64 = 8;

/// The largest chunk of a discarded range read or written at once.
const ZERO_CHUNK: usize = 1 << 20;

/// What a log entry records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Sectors written, with their data in the log. A zero write is a
    /// write of zeros.
    Write,
    /// Sectors discarded: they read as zeros after it.
    Discard,
    /// The guest asked the disk to make every earlier write permanent.
    Flush,
    /// A mark of the recorder's own, which changes nothing.
    Mark,
}

/// One entry of the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The first sector of the disk it concerns.
    pub sector: u64,
    /// How many sectors it concerns.
    pub sectors: u64,
    /// What it records.
    pub kind: Kind,
    /// The offset in the log file of the data of a write.
    pub data: u64,
}

/// A write log, read from its file.
pub struct Log {
    file: File,
    sector_size: u64,
    entries: Vec<Entry>,
}

/// The fields of the log's superblock that readers use.
struct Superblock {
    entries: u64,
    sector_size: u64,
}

fn read_superblock(file: &File, path: &Path) -> Result<Superblock> {
    let mut block = [0u8; SUPERBLOCK];
    file.read_exact_at(&mut block, 0)
        .map_err(|err| Error::new(format!("{} has no log superblock: {err}", path.display())))?;
    let field = |at: usize| u64::from_le_bytes(block[at..at + 8].try_into().expect("8 bytes"));
    if field(0) != MAGIC {
        crate::bail!("{} is not a write log: its magic number is wrong", path.display());
    }
    if field(8) != VERSION {
        crate::bail!(
            "{} is version {} of the write log; only version {VERSION} is read",
            path.display(),
            field(8)
        );
    }
    let sector_size = u64::from(u32::from_le_bytes(block[24..28].try_into().expect("4 bytes")));
    if !sector_size.is_power_of_two() || !(512..=16 << 20).contains(&sector_size) {
        crate::bail!(
            "{} has sectors of {sector_size} bytes, which is not a size of 512 bytes to 16 MiB",
            path.display()
        );
    }
    Ok(Superblock { entries: field(16), sector_size })
}

impl Log {
    /// Reads the log at `path`: the entries its superblock counts, and
    /// not any that QEMU wrote after the last count it saved.
    ///
    /// # Errors
    ///
    /// If the file is not a write log of a supported version, or an
    /// entry, or the data of one, runs past the end of the file.
    pub fn open(path: &Path) -> Result<Log> {
        let file = File::open(path).context(|| format!("opening {}", path.display()))?;
        let Superblock { entries: count, sector_size } = read_superblock(&file, path)?;
        let length = file.metadata().context(|| format!("reading {}", path.display()))?.len();
        let mut entries = Vec::new();
        let mut place = sector_size;
        for index in 0..count {
            let mut head = [0u8; ENTRY_HEAD];
            file.read_exact_at(&mut head, place).map_err(|_| {
                Error::new(format!(
                    "entry {index} of {} is missing: the file ends first",
                    path.display()
                ))
            })?;
            let field =
                |at: usize| u64::from_le_bytes(head[at..at + 8].try_into().expect("8 bytes"));
            let (sector, sectors, flags) = (field(0), field(8), field(16));
            if flags & !(FLAG_FLUSH | FLAG_FUA | FLAG_DISCARD | FLAG_MARK) != 0 {
                crate::bail!(
                    "entry {index} of {} has flags {flags:#x}, which are not known",
                    path.display()
                );
            }
            let kind = if flags & FLAG_DISCARD != 0 {
                Kind::Discard
            } else if flags & FLAG_MARK != 0 {
                Kind::Mark
            } else if sectors == 0 && flags & FLAG_FLUSH != 0 {
                Kind::Flush
            } else {
                Kind::Write
            };
            let data = place + sector_size;
            let data_bytes = match kind {
                Kind::Discard | Kind::Flush => 0,
                Kind::Write | Kind::Mark => sectors.saturating_mul(sector_size),
            };
            let end = data.saturating_add(data_bytes);
            if end > length {
                crate::bail!(
                    "entry {index} of {} ({sectors} sectors at sector {sector}) has data that runs past the end of the file",
                    path.display()
                );
            }
            entries.push(Entry { sector, sectors, kind, data });
            place = end;
        }
        Ok(Log { file, sector_size, entries })
    }

    /// How many entries the superblock of the log at `path` counts at
    /// this moment, for a mark while QEMU still writes it. QEMU saves
    /// the count at each flush, so a `sync` in the guest makes it exact.
    ///
    /// # Errors
    ///
    /// If the file has no readable superblock.
    pub fn entries_at_superblock(path: &Path) -> Result<u64> {
        let file = File::open(path).context(|| format!("opening {}", path.display()))?;
        Ok(read_superblock(&file, path)?.entries)
    }

    /// The indices of the flushes among the entries in `range`.
    pub fn flushes(&self, range: Range<usize>) -> Vec<usize> {
        let end = range.end.min(self.entries.len());
        (range.start..end).filter(|&at| self.entries[at].kind == Kind::Flush).collect()
    }

    /// The data of write `entry`.
    fn data(&self, entry: &Entry) -> Result<Vec<u8>> {
        let length = usize::try_from(entry.sectors * self.sector_size)
            .map_err(|_| Error::new("a write of the log is larger than memory"))?;
        let mut data = vec![0u8; length];
        self.file
            .read_exact_at(&mut data, entry.data)
            .context(|| format!("reading the data of the write at sector {}", entry.sector))?;
        Ok(data)
    }
}

/// A copy of a disk image, onto which the writes of a [`Log`] are put.
pub struct Disk {
    file: File,
    path: PathBuf,
}

impl Disk {
    /// Copies the image `from` to `to`, keeping its holes: a copy with
    /// `std::io::copy` writes every hole of a multi-gigabyte image out as
    /// zeros. `cp` shares the data blocks of the two files where the
    /// filesystem allows it.
    ///
    /// # Errors
    ///
    /// If `cp` cannot run or fails, or `to` cannot be opened after it.
    pub fn copy_of(from: &Path, to: &Path) -> Result<Disk> {
        let mut cp = Command::new("cp");
        cp.args(["--sparse=always", "--reflink=auto", "--"]).arg(from).arg(to);
        let output = cp.output().map_err(|err| process::spawn_error("cp", &err))?;
        if !output.status.success() {
            crate::bail!(
                "copying {} to {} failed: {}",
                from.display(),
                to.display(),
                String::from_utf8_lossy(&output.stderr).trim_end()
            );
        }
        let file = File::options()
            .read(true)
            .write(true)
            .open(to)
            .context(|| format!("opening {}", to.display()))?;
        Ok(Disk { file, path: to.to_path_buf() })
    }

    /// The file of the disk.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Puts the writes and discards of the entries in `range` on the
    /// disk, in log order. Flushes and marks change nothing.
    ///
    /// # Errors
    ///
    /// If the log's data cannot be read or the disk cannot be written.
    pub fn apply(&self, log: &Log, range: Range<usize>) -> Result<()> {
        let end = range.end.min(log.entries.len());
        (range.start..end).try_for_each(|at| self.apply_entry(log, at))
    }

    /// Puts the entries at `indices` on the disk, in the order given.
    ///
    /// # Errors
    ///
    /// As [`Disk::apply`].
    pub fn apply_each(&self, log: &Log, indices: &[usize]) -> Result<()> {
        indices.iter().try_for_each(|&at| self.apply_entry(log, at))
    }

    fn apply_entry(&self, log: &Log, at: usize) -> Result<()> {
        let entry = &log.entries[at];
        let offset = entry.sector.checked_mul(log.sector_size).ok_or_else(|| {
            Error::new(format!("entry {at} of the log is at a sector beyond any disk"))
        })?;
        match entry.kind {
            Kind::Write => self
                .file
                .write_all_at(&log.data(entry)?, offset)
                .context(|| format!("writing entry {at} to {}", self.path.display())),
            Kind::Discard => self.zero(offset, entry.sectors.saturating_mul(log.sector_size)),
            Kind::Flush | Kind::Mark => Ok(()),
        }
    }

    /// Makes `length` bytes at `offset` read as zeros, writing only
    /// where the disk holds something else, so that a discard of a range
    /// the copy keeps as a hole costs no space.
    fn zero(&self, offset: u64, length: u64) -> Result<()> {
        let mut chunk = vec![0u8; ZERO_CHUNK];
        let end = offset.saturating_add(length);
        let mut at = offset;
        while at < end {
            let size = usize::try_from(end - at).map_or(ZERO_CHUNK, |rest| rest.min(ZERO_CHUNK));
            let held = self
                .file
                .read_at(&mut chunk[..size], at)
                .context(|| format!("reading {}", self.path.display()))?;
            if held == 0 {
                // Past the end of the image, which reads as zeros already.
                return Ok(());
            }
            if chunk[..held].iter().any(|&byte| byte != 0) {
                self.file
                    .write_all_at(&vec![0u8; held], at)
                    .context(|| format!("writing zeros to {}", self.path.display()))?;
            }
            at += held as u64;
        }
        Ok(())
    }
}

/// What a power cut can leave on the disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cut {
    /// Every write logged before flush `.0`, which the disk had made
    /// permanent, and none after it.
    AtFlush(usize),
    /// Everything up to flush `after`, and of the writes between it and
    /// the next flush, `upto`, only those at the entries of `keep`: a
    /// disk may make any of the writes it was not told to flush permanent,
    /// in any combination. The entries are in log order.
    Within { after: usize, upto: usize, keep: Vec<usize> },
}

impl Cut {
    /// The flush the cut is at or after: the entries before it are on the
    /// disk whole.
    pub fn flush(&self) -> usize {
        match self {
            Cut::AtFlush(at) => *at,
            Cut::Within { after, .. } => *after,
        }
    }

    /// What kind of cut it is, for a line of output.
    pub fn kind(&self) -> &'static str {
        match self {
            Cut::AtFlush(_) => "flush",
            Cut::Within { .. } => "part of a flush interval",
        }
    }
}

/// How many of the cuts of a range to take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Points {
    /// Every one.
    All,
    /// This many, drawn by the seed, among them the first and the last
    /// flush of the range.
    Sample(usize),
}

/// A seed for a run that was not given one: the clock's nanoseconds.
pub fn clock_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_nanos() & u128::from(u64::MAX)).unwrap_or(0))
}

/// A small pseudo-random source (SplitMix64), so that a run can be
/// repeated from the seed it printed.
pub struct Random(u64);

impl Random {
    /// A source that gives the same numbers for the same `seed`.
    pub fn new(seed: u64) -> Random {
        Random(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `bound`, which is more than 0.
    pub fn below(&mut self, bound: u64) -> u64 {
        // The bias of a remainder is under 2^-32 for the bounds used here.
        self.next() % bound
    }

    /// Picks `count` of `items`, by a shuffle that stops once it has them.
    fn pick<T: Copy>(&mut self, items: &[T], count: usize) -> Vec<T> {
        let mut pool = items.to_vec();
        for at in 0..count.min(pool.len()) {
            let other = at + usize::try_from(self.below((pool.len() - at) as u64)).unwrap_or(0);
            pool.swap(at, other);
        }
        pool.truncate(count);
        pool
    }
}

/// The writes and discards of the log between flush `after` and flush
/// `upto`, drawn as a part of them, if there are two or more.
fn partial_writes(log: &Log, after: usize, upto: usize, random: &mut Random) -> Option<Vec<usize>> {
    let writes: Vec<usize> = (after + 1..upto)
        .filter(|&at| matches!(log.entries[at].kind, Kind::Write | Kind::Discard))
        .collect();
    if writes.len() < 2 {
        return None;
    }
    // At least one, and not all.
    let count = 1 + usize::try_from(random.below(writes.len() as u64 - 1)).unwrap_or(0);
    let mut keep = random.pick(&writes, count);
    keep.sort_unstable();
    Some(keep)
}

/// The cuts of the entries in `range`, in log order, each with its number
/// among all the cuts of that range for that seed: the flush points in
/// it, and a cut between two flushes for each interval that holds two or
/// more writes. `how` takes all of them or a sample, which keeps the
/// range's first and last flush. The subsets of a cut between flushes are
/// the same for a seed whatever `how` is, so a number names the same cut
/// in a sampled run and in a run of one.
pub fn cuts(log: &Log, range: Range<usize>, how: Points, seed: u64) -> Vec<(usize, Cut)> {
    let mut random = Random::new(seed);
    let flushes = log.flushes(range.clone());
    let before = log.flushes(0..range.start).last().copied();
    let mut all = Vec::new();
    for (place, &flush) in flushes.iter().enumerate() {
        let previous = if place == 0 { before } else { Some(flushes[place - 1]) };
        if let Some(after) = previous
            && let Some(keep) = partial_writes(log, after, flush, &mut random)
        {
            all.push(Cut::Within { after, upto: flush, keep });
        }
        all.push(Cut::AtFlush(flush));
    }
    let numbered: Vec<(usize, Cut)> = all.into_iter().enumerate().collect();
    let Points::Sample(count) = how else {
        return numbered;
    };
    if count >= numbered.len() {
        return numbered;
    }
    let flush_points: Vec<usize> = numbered
        .iter()
        .filter(|(_, cut)| matches!(cut, Cut::AtFlush(_)))
        .map(|&(number, _)| number)
        .collect();
    let mut chosen: Vec<usize> = Vec::new();
    for edge in [flush_points.first(), flush_points.last()].into_iter().flatten() {
        if chosen.len() < count && !chosen.contains(edge) {
            chosen.push(*edge);
        }
    }
    let others: Vec<usize> =
        (0..numbered.len()).filter(|number| !chosen.contains(number)).collect();
    chosen.extend(random.pick(&others, count - chosen.len()));
    chosen.sort_unstable();
    numbered.into_iter().filter(|(number, _)| chosen.binary_search(number).is_ok()).collect()
}

/// Part `part` of `parts` of `items`: every `parts`-th one, starting at
/// the one numbered `part` from 1. The parts of one list share it out with
/// no item in two parts.
pub fn part_of<T>(items: Vec<T>, part: usize, parts: usize) -> Vec<T> {
    items
        .into_iter()
        .enumerate()
        .filter(|(at, _)| at % parts == part - 1)
        .map(|(_, item)| item)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECTOR: usize = 512;

    /// A log built entry by entry in the format QEMU writes.
    struct Builder {
        bytes: Vec<u8>,
        entries: u64,
    }

    impl Builder {
        fn new() -> Builder {
            Builder { bytes: vec![0; SECTOR], entries: 0 }
        }

        fn entry(&mut self, sector: u64, sectors: u64, flags: u64, data: &[u8]) {
            let mut head = vec![0u8; SECTOR];
            head[0..8].copy_from_slice(&sector.to_le_bytes());
            head[8..16].copy_from_slice(&sectors.to_le_bytes());
            head[16..24].copy_from_slice(&flags.to_le_bytes());
            self.bytes.extend(head);
            self.bytes.extend(data);
            self.entries += 1;
        }

        fn write(&mut self, sector: u64, fill: u8, sectors: u64) {
            self.entry(sector, sectors, 0, &vec![fill; sectors as usize * SECTOR]);
        }

        fn discard(&mut self, sector: u64, sectors: u64) {
            self.entry(sector, sectors, FLAG_DISCARD, &[]);
        }

        fn flush(&mut self) {
            self.entry(0, 0, FLAG_FLUSH, &[]);
        }

        /// The file, with the superblock counting `counted` entries.
        fn save(&mut self, name: &str, counted: u64) -> PathBuf {
            self.bytes[0..8].copy_from_slice(&MAGIC.to_le_bytes());
            self.bytes[8..16].copy_from_slice(&VERSION.to_le_bytes());
            self.bytes[16..24].copy_from_slice(&counted.to_le_bytes());
            self.bytes[24..28].copy_from_slice(&(SECTOR as u32).to_le_bytes());
            let path = scratch(name);
            std::fs::write(&path, &self.bytes).unwrap();
            path
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ob-writelog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// A log of a write, a flush, three writes, a flush, a write, a flush.
    fn sample_log(name: &str) -> Log {
        let mut log = Builder::new();
        log.write(0, 1, 1);
        log.flush();
        log.write(1, 2, 1);
        log.write(2, 3, 1);
        log.write(3, 4, 1);
        log.flush();
        log.write(4, 5, 1);
        log.flush();
        let count = log.entries;
        Log::open(&log.save(name, count)).unwrap()
    }

    fn disk_of(name: &str, sectors: usize, fill: u8) -> Disk {
        let base = scratch(&format!("{name}-base.img"));
        std::fs::write(&base, vec![fill; sectors * SECTOR]).unwrap();
        Disk::copy_of(&base, &scratch(&format!("{name}.img"))).unwrap()
    }

    fn sector_of(disk: &Disk, sector: u64) -> Vec<u8> {
        let mut bytes = vec![0u8; SECTOR];
        disk.file.read_exact_at(&mut bytes, sector * SECTOR as u64).unwrap();
        bytes
    }

    #[test]
    fn a_log_of_every_kind_of_entry_is_read() {
        let mut log = Builder::new();
        log.write(8, 7, 2);
        // A zero write is a write of zeros.
        log.entry(20, 1, 0, &[0; SECTOR]);
        log.discard(100, 50);
        log.flush();
        log.entry(0, 0, FLAG_FUA | FLAG_FLUSH, &[]);
        let count = log.entries;
        let log = Log::open(&log.save("kinds.log", count)).unwrap();
        let kinds: Vec<Kind> = log.entries.iter().map(|entry| entry.kind).collect();
        assert_eq!(kinds, [Kind::Write, Kind::Write, Kind::Discard, Kind::Flush, Kind::Flush]);
        let first = log.entries[0];
        assert_eq!((first.sector, first.sectors, first.data), (8, 2, 2 * SECTOR as u64));
        // The next entry's description follows the two sectors of data.
        assert_eq!(log.entries[1].data, 5 * SECTOR as u64);
        // A discard has no data, so the entry after it is next door.
        assert_eq!(log.entries[3].data, log.entries[2].data + SECTOR as u64);
    }

    #[test]
    fn entries_after_the_counted_ones_are_not_read() {
        let mut log = Builder::new();
        log.write(0, 1, 1);
        log.flush();
        log.write(1, 2, 1);
        let path = log.save("stale.log", 2);
        assert_eq!(Log::open(&path).unwrap().entries.len(), 2);
        assert_eq!(Log::entries_at_superblock(&path).unwrap(), 2);
    }

    #[test]
    fn a_file_that_is_not_a_log_is_refused() {
        let mut log = Builder::new();
        log.flush();
        let path = log.save("magic.log", 1);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        let Err(why) = Log::open(&path) else { panic!("a wrong magic number was taken") };
        assert!(why.to_string().contains("not a write log"), "{why}");
        assert!(Log::entries_at_superblock(&path).is_err());
    }

    #[test]
    fn flags_that_are_not_known_are_refused_and_fua_and_mark_are_not() {
        let mut log = Builder::new();
        log.entry(0, 1, 16, &[0; SECTOR]);
        let path = log.save("flags.log", 1);
        let Err(why) = Log::open(&path) else { panic!("an unknown flag was taken") };
        assert!(
            why.to_string().contains("entry 0") && why.to_string().contains("flags 0x10"),
            "{why}"
        );

        let mut log = Builder::new();
        log.entry(5, 1, FLAG_FUA, &[1; SECTOR]);
        log.entry(0, 1, FLAG_MARK, &[0; SECTOR]);
        let log = Log::open(&log.save("marks.log", 2)).unwrap();
        assert_eq!(log.entries[0].kind, Kind::Write);
        assert_eq!(log.entries[1].kind, Kind::Mark);
    }

    #[test]
    fn data_that_runs_past_the_end_of_the_file_names_its_entry() {
        let mut log = Builder::new();
        log.write(0, 1, 1);
        log.write(9, 2, 2);
        let path = log.save("short.log", 2);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
        let Err(why) = Log::open(&path) else { panic!("a short log was taken") };
        assert!(
            why.to_string().contains("entry 1") && why.to_string().contains("at sector 9"),
            "{why}"
        );
        // A count that goes past the last entry is cut short as well.
        let mut log = Builder::new();
        log.flush();
        let Err(why) = Log::open(&log.save("many.log", 3)) else {
            panic!("a long count was taken")
        };
        assert!(why.to_string().contains("entry 1 "), "{why}");
    }

    #[test]
    fn sectors_of_other_sizes_are_checked() {
        for (size, good) in [
            (512u32, true),
            (4096, true),
            (16 << 20, true),
            (256, false),
            (1000, false),
            (32 << 20, false),
        ] {
            let mut log = Builder::new();
            let path = log.save("size.log", 0);
            let mut bytes = std::fs::read(&path).unwrap();
            bytes[24..28].copy_from_slice(&size.to_le_bytes());
            std::fs::write(&path, &bytes).unwrap();
            assert_eq!(Log::open(&path).is_ok(), good, "sectors of {size} bytes");
        }
    }

    #[test]
    fn flushes_are_found_in_a_range() {
        let log = sample_log("flushes.log");
        assert_eq!(log.flushes(0..log.entries.len()), [1, 5, 7]);
        assert_eq!(log.flushes(2..7), [5]);
        assert_eq!(log.flushes(2..100), [5, 7]);
    }

    #[test]
    fn applying_a_log_puts_its_writes_on_the_disk() {
        let log = sample_log("apply.log");
        let disk = disk_of("apply", 8, 9);
        disk.apply(&log, 0..6).unwrap();
        let sectors: Vec<u8> = (0..6).map(|sector| sector_of(&disk, sector)[0]).collect();
        assert_eq!(sectors, [1, 2, 3, 4, 9, 9]);
        // The rest, from where the first part ended.
        disk.apply(&log, 6..8).unwrap();
        assert_eq!(sector_of(&disk, 4)[0], 5);
        assert_eq!(sector_of(&disk, 4)[SECTOR - 1], 5);
    }

    #[test]
    fn a_discard_reads_back_as_zeros() {
        let mut log = Builder::new();
        log.write(2, 7, 1);
        log.discard(1, 3);
        log.discard(6, 100);
        let count = log.entries;
        let log = Log::open(&log.save("discard.log", count)).unwrap();
        let disk = disk_of("discard", 8, 9);
        disk.apply(&log, 0..1).unwrap();
        assert_eq!(sector_of(&disk, 2)[0], 7);
        disk.apply(&log, 1..3).unwrap();
        let sectors: Vec<u8> = (0..8).map(|sector| sector_of(&disk, sector)[0]).collect();
        assert_eq!(sectors, [9, 0, 0, 0, 9, 9, 0, 0]);
        // The discard past the end of the image leaves the image's size.
        assert_eq!(std::fs::metadata(disk.path()).unwrap().len(), 8 * SECTOR as u64);
    }

    #[test]
    fn a_copy_keeps_the_holes_of_the_image() {
        use std::os::unix::fs::MetadataExt;
        let base = scratch("holes-base.img");
        let file = File::create(&base).unwrap();
        file.set_len(64 << 20).unwrap();
        file.write_all_at(b"data", 0).unwrap();
        let disk = Disk::copy_of(&base, &scratch("holes.img")).unwrap();
        let meta = std::fs::metadata(disk.path()).unwrap();
        assert_eq!(meta.len(), 64 << 20);
        assert!(
            meta.blocks() * 512 < 1 << 20,
            "the copy of a 64 MiB hole holds {} blocks",
            meta.blocks()
        );
    }

    #[test]
    fn every_flush_is_a_cut_and_so_is_a_part_of_each_interval_with_two_writes() {
        let log = sample_log("cuts.log");
        let all = cuts(&log, 0..8, Points::All, 7);
        // The interval between flushes 1 and 5 holds three writes, the
        // others fewer than two.
        let kinds: Vec<&Cut> = all.iter().map(|(_, cut)| cut).collect();
        assert_eq!(kinds.len(), 4);
        assert_eq!(*kinds[0], Cut::AtFlush(1));
        let Cut::Within { after: 1, upto: 5, keep } = kinds[1] else { panic!("{:?}", kinds[1]) };
        assert_eq!(*kinds[2], Cut::AtFlush(5));
        assert_eq!(*kinds[3], Cut::AtFlush(7));
        assert!((1..3).contains(&keep.len()), "a part of three writes is {keep:?}");
        assert!(keep.iter().all(|at| (2..5).contains(at)));
        assert!(keep.windows(2).all(|pair| pair[0] < pair[1]));
        let numbers: Vec<usize> = all.iter().map(|&(number, _)| number).collect();
        assert_eq!(numbers, [0, 1, 2, 3]);
    }

    #[test]
    fn a_part_of_an_interval_is_never_empty_and_never_whole() {
        let mut log = Builder::new();
        log.flush();
        for sector in 0..6 {
            log.write(sector, 1, 1);
        }
        log.flush();
        let count = log.entries;
        let log = Log::open(&log.save("partial.log", count)).unwrap();
        for seed in 0..200 {
            let all = cuts(&log, 0..8, Points::All, seed);
            let Cut::Within { keep, .. } = &all[1].1 else { panic!("no cut between flushes") };
            assert!((1..6).contains(&keep.len()), "seed {seed}: {keep:?}");
        }
    }

    #[test]
    fn the_same_seed_makes_the_same_cuts() {
        let log = sample_log("seeded.log");
        assert_eq!(cuts(&log, 0..8, Points::All, 3), cuts(&log, 0..8, Points::All, 3));
        let kept = |seed| match &cuts(&log, 0..8, Points::All, seed)[1].1 {
            Cut::Within { keep, .. } => keep.clone(),
            other => panic!("{other:?}"),
        };
        let parts: std::collections::HashSet<Vec<usize>> = (0..40).map(kept).collect();
        assert!(parts.len() > 1, "every seed drew the same part");
    }

    #[test]
    fn a_sample_keeps_the_first_and_last_flush_and_the_log_order() {
        let mut log = Builder::new();
        for round in 0..30 {
            log.write(round, 1, 1);
            log.write(round + 1, 2, 1);
            log.flush();
        }
        let count = log.entries;
        let log = Log::open(&log.save("sample.log", count)).unwrap();
        let end = log.entries.len();
        let all = cuts(&log, 0..end, Points::All, 11);
        for seed in 0..20 {
            let sample = cuts(&log, 0..end, Points::Sample(8), seed);
            assert_eq!(sample.len(), 8, "seed {seed}");
            assert_eq!(sample[0].1, Cut::AtFlush(2));
            assert_eq!(sample[7].1, Cut::AtFlush(end - 1));
            assert!(sample.windows(2).all(|pair| pair[0].0 < pair[1].0));
            assert_eq!(sample, cuts(&log, 0..end, Points::Sample(8), seed));
            // A cut has one number, whatever is sampled.
            let again = cuts(&log, 0..end, Points::All, seed);
            for (number, cut) in &sample {
                assert_eq!(&again[*number].1, cut, "seed {seed}");
            }
        }
        assert_eq!(cuts(&log, 0..end, Points::Sample(1000), 11), all);
        let two = cuts(&log, 0..end, Points::Sample(2), 11);
        assert_eq!(two.len(), 2);
        assert_eq!(cuts(&log, 0..end, Points::Sample(1), 11).len(), 1);
    }

    #[test]
    fn a_range_starts_its_first_interval_at_the_flush_before_it() {
        let log = sample_log("range.log");
        // From entry 2: the writes after flush 1 are the first interval.
        let all = cuts(&log, 2..8, Points::All, 5);
        assert!(matches!(all[0].1, Cut::Within { after: 1, upto: 5, .. }));
        assert_eq!(all[1].1, Cut::AtFlush(5));
        assert_eq!(all[0].1.flush(), 1);
        // With no flush before the range, there is no interval before its first flush.
        let all = cuts(&log, 0..8, Points::All, 5);
        assert_eq!(all[0].1, Cut::AtFlush(1));
    }

    #[test]
    fn a_part_takes_every_part_th_cut() {
        let items: Vec<u32> = (0..10).collect();
        assert_eq!(part_of(items.clone(), 1, 3), [0, 3, 6, 9]);
        assert_eq!(part_of(items.clone(), 2, 3), [1, 4, 7]);
        assert_eq!(part_of(items.clone(), 3, 3), [2, 5, 8]);
        assert_eq!(part_of(items, 1, 1).len(), 10);
    }

    #[test]
    fn a_cut_within_two_flushes_puts_only_the_kept_writes_on_the_disk() {
        let log = sample_log("within.log");
        let disk = disk_of("within", 8, 9);
        // Everything up to flush 1, then the writes at entries 2 and 4.
        disk.apply(&log, 0..2).unwrap();
        disk.apply_each(&log, &[2, 4]).unwrap();
        let sectors: Vec<u8> = (0..5).map(|sector| sector_of(&disk, sector)[0]).collect();
        assert_eq!(sectors, [1, 2, 9, 4, 9]);
    }
}
