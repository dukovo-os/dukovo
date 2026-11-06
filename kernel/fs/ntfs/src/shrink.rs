// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Shrinking a volume (ADR 0312), to make room beside Windows for
//! Dukovo, as `ntfsresize` does for Linux's installers.
//!
//! A volume shrinks from its end. The clusters past the new end that
//! files hold are copied below it, into free ones, and each file's record
//! is changed to say where its copies are. Then the boot sector says the
//! new size, `$Bitmap` and `$BadClus` are cut to it, and the copy of the
//! boot sector goes in the new last sector. The partition is shrunk
//! after, by the partitioner: the volume no longer uses what it gives up.
//!
//! # What it refuses
//!
//! - A volume Radix would not write ([`crate::fs`]): Windows must have
//!   shut down fully, with no hibernation or Fast Startup, nothing left
//!   in its journal, and nothing marked to be checked.
//! - A volume the check ([`crate::check`]) finds unsound: moving a
//!   cluster two files hold, or one `$Bitmap` calls free, would harm a
//!   file.
//! - A size under the smallest ([`Info::smallest`]): what the files hold
//!   and a margin must fit below the new end, and the master file table's
//!   first records stay where the boot sector says they are.
//! - A file whose runs would no longer fit in its record once moved, as
//!   a file in very many pieces moved into many small gaps.
//!
//! Every move is worked out, and every record checked to hold its new
//! runs, before anything is written: a refusal changes nothing.
//!
//! # A power cut
//!
//! Nothing a record points at is overwritten while it points there, so a
//! power cut at any moment leaves every file's data where its record
//! says, and Radix mounts the volume to read it. The order:
//!
//! 1. The volume is marked dirty, and the journal emptied, as before any
//!    change Radix makes: after a cut, `chkdsk` checks it.
//! 2. The clusters the copies go to are marked in use.
//! 3. The files' clusters are copied there, and the device flushed. A cut
//!    until step 4 leaves those clusters marked but no file's: lost,
//!    which `chkdsk` gives back.
//! 4. Each file's record is rewritten to say where its copies are, and
//!    the device flushed. A record written points at whole copies; one
//!    not yet at the clusters it always had, which nothing has touched.
//! 5. The master file table's own clusters past the end are copied, with
//!    the records as step 4 left them, then its first record, which never
//!    moves, says where.
//! 6. The clusters copied from are marked free: the volume, still of its
//!    old size, is whole again.
//! 7. `$Bitmap`'s clusters past the end, if any, are copied below it,
//!    now that no other bit changes, and its record says where.
//! 8. The boot sector says the new size: one sector's write. Until step
//!    9 `$Bitmap` is longer than the volume needs, which costs nothing.
//! 9. `$Bitmap` and `$BadClus` are cut to the new size.
//! 10. The copy of the boot sector is written in the new last sector.
//!
//! A record is written in one request. A cut in the middle of it, which a
//! disk of 4 KiB physical sectors does not leave, makes its update
//! sequence refuse it, as for any record Radix writes (ADR 0238).
//!
//! The volume stays marked dirty at the end, as `ntfsresize` leaves it,
//! so that Windows checks it at its next start, unless asked not to
//! ([`Options::check_at_windows_start`]).

use core::cmp::Reverse;
use core::fmt;
use core::sync::atomic::Ordering;
use std::collections::BTreeSet;
use std::vec;
use std::vec::Vec;

use radix_block::mem::CachePage;
use radix_block::{DeviceRef, PAGE_SIZE, SECTOR, Segment, SegmentMut};
use radix_vfs::mem::KVec;
use radix_vfs::{Error, Result};

use crate::NtfsFs;
use crate::boot::Geometry;
use crate::check::{self, Report, is_bad_clusters};
use crate::edit::{self, put_u64};
use crate::fs::{Inner, Volume, records, zeroed};
use crate::record::{Attr, Record, attr_flags, kind, u16_at, u32_at, u64_at};
use crate::runs::{HOLE, Run, Runs};
use crate::space::Bits;

/// The bytes of `$Boot`, the volume's first clusters: its boot sector and
/// the code after, which never move.
const BOOT_BYTES: u64 = 8192;

/// `$Bad`, the name of `$BadClus`'s stream of bad clusters, as UTF-16
/// bytes.
const BAD: [u8; 8] = [b'$', 0, b'B', 0, b'a', 0, b'd', 0];

/// What a volume is, for a shrink.
#[derive(Debug)]
pub struct Info {
    /// Its size: the partition it fills, the last sector of which holds
    /// the copy of its boot sector.
    pub bytes: u64,
    /// The smallest size it shrinks to, in whole MiB: the clusters its
    /// files hold, bad ones aside, and a margin of a hundredth of the
    /// volume for Windows to work in, with the copy of the boot sector.
    pub smallest: u64,
    /// Why Radix would not write it, and so would not shrink it.
    pub kept: Option<&'static str>,
    /// Whether Windows hibernated with it, or shut down with Fast
    /// Startup.
    pub hibernated: bool,
    /// Whether it is marked to be checked.
    pub dirty: bool,
    /// What the check found.
    pub report: Report,
}

/// How to shrink a volume.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// The size to shrink it to, in bytes: the partition it will fill,
    /// the last sector of which takes the copy of its boot sector.
    pub bytes: u64,
    /// Working out what would be moved, and writing nothing.
    pub dry_run: bool,
    /// Leaving it marked dirty, so that Windows checks it at its next
    /// start, as after `ntfsresize`.
    pub check_at_windows_start: bool,
}

/// How far a shrink is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Checking the volume, and finding what lies past the new end.
    Checking,
    /// Copying clusters below the new end.
    Moving {
        /// The bytes copied.
        done: u64,
        /// The bytes to copy in all.
        total: u64,
    },
    /// Checking the shrunk volume.
    Rechecking,
}

/// What a shrink did.
#[derive(Debug)]
pub struct Shrunk {
    /// The volume's new size: the partition it fills, the copy of its
    /// boot sector in the last sector.
    pub bytes: u64,
    /// The bytes copied below the new end.
    pub moved: u64,
    /// The records rewritten to say where their clusters went.
    pub records: usize,
    /// What the check of the shrunk volume found; on a dry run, what the
    /// check before found.
    pub report: Report,
}

/// Why a volume was not shrunk. Only [`Refused::Failed`] can come once
/// something is written.
#[derive(Debug)]
pub enum Refused {
    /// It could not be read or written.
    Failed(Error),
    /// Radix would not write it ([`NtfsFs::kept_from_writing`]): why.
    Kept(&'static str),
    /// The check found it unsound: what it found.
    Unsound(Report),
    /// The size asked for is under the smallest it shrinks to
    /// ([`Info::smallest`]): that.
    TooSmall(u64),
    /// The size asked for is not under its size.
    NotSmaller,
    /// The runs of the file in this record would not fit in it once
    /// moved: it is in too many pieces.
    Fragmented(u64),
}

impl From<Error> for Refused {
    fn from(err: Error) -> Refused {
        Refused::Failed(err)
    }
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Failed(Error::Io) => f.write_str("the disk could not be read or written"),
            Refused::Failed(Error::Corrupt) => f.write_str("the volume is damaged"),
            Refused::Failed(Error::UnknownFilesystem) => f.write_str("it holds no NTFS volume"),
            Refused::Failed(Error::NotSupported) => {
                f.write_str("the volume uses what Dukovo does not support")
            }
            Refused::Failed(err) => write!(f, "the volume could not be shrunk ({err:?})"),
            Refused::Kept(why) => f.write_str(why),
            Refused::Unsound(report) => {
                write!(f, "the check found {} problems", report.count)?;
                if let Some(first) = report.problems.first() {
                    write!(f, ", the first: {first}")?;
                }
                f.write_str("; let Windows mend it (chkdsk /f) first")
            }
            Refused::TooSmall(smallest) => {
                write!(f, "it shrinks to {} MiB at the least", smallest >> 20)
            }
            Refused::NotSmaller => f.write_str("the size asked for is not under the volume's"),
            Refused::Fragmented(record) => write!(
                f,
                "the file in record {record} is in too many pieces to move; let Windows optimize \
                 (defragment) the drive first"
            ),
        }
    }
}

/// What the volume on `device` is, for a shrink: its size, the smallest
/// it shrinks to, why it would not be, and what the check finds.
pub fn info(device: DeviceRef) -> core::result::Result<Info, Refused> {
    let fs = NtfsFs::mount(device, true)?;
    let inner = &fs.inner;
    let geo = inner.vol.geo;
    let bitmap = inner.space.lock().clusters.bitmap.copy()?;
    let report = check::walk(&inner.vol, &bitmap, &mut |_, _, _| {})?;
    Ok(Info {
        bytes: (boot_sectors(&inner.vol)? + 1) * u64::from(geo.sector_size),
        smallest: smallest(&geo, &report),
        kept: fs.kept_from_writing(),
        hibernated: fs.hibernated(),
        dirty: fs.dirty(),
        report,
    })
}

/// Shrinks the volume on `device` as `options` say, telling `progress`
/// how far it is.
pub fn shrink(
    device: DeviceRef,
    options: Options,
    progress: &mut dyn FnMut(Step),
) -> core::result::Result<Shrunk, Refused> {
    // A mount that writes nothing says why Radix would not write it.
    if let Some(why) = NtfsFs::mount(device.clone(), true)?.kept_from_writing() {
        return Err(Refused::Kept(why));
    }
    let fs = NtfsFs::mount(device.clone(), options.dry_run)?;
    let inner = &fs.inner;
    let geo = inner.vol.geo;
    let target = Target::new(&geo, options.bytes);
    if target.sectors >= boot_sectors(&inner.vol)? {
        return Err(Refused::NotSmaller);
    }
    progress(Step::Checking);
    let bitmap = inner.space.lock().clusters.bitmap.copy()?;
    let mut past = Past::new(target);
    let report = check::walk(&inner.vol, &bitmap, &mut |number, attr, run| {
        past.see(number, attr, run);
    })?;
    if !report.sound() {
        return Err(Refused::Unsound(report));
    }
    let smallest = smallest(&geo, &report);
    if options.bytes < smallest {
        return Err(Refused::TooSmall(smallest));
    }
    let plan = Plan::make(&inner.vol, &bitmap, past, smallest)?;
    let moved = plan.moves.iter().map(|m| m.len).sum::<u64>() * u64::from(geo.cluster_size);
    let mut shrunk =
        Shrunk { bytes: target.bytes(&geo), moved, records: plan.records.len(), report };
    if options.dry_run {
        return Ok(shrunk);
    }
    progress(Step::Moving { done: 0, total: moved });
    plan.carry_out(inner, progress)?;
    if !options.check_at_windows_start {
        inner.keep_dirty.store(false, Ordering::Relaxed);
        inner.sync_all()?;
    }
    drop(fs);
    progress(Step::Rechecking);
    shrunk.report = check::check(device)?;
    Ok(shrunk)
}

/// How many sectors the boot sector of `vol` says the volume has.
fn boot_sectors(vol: &Volume) -> Result<u64> {
    let mut boot = [0u8; 512];
    vol.cache.read(0, &mut boot)?;
    Ok(u64_at(&boot, 0x28))
}

/// The smallest size, in bytes, a volume of `geo` that the check found
/// as `report` says shrinks to ([`Info::smallest`]).
fn smallest(geo: &Geometry, report: &Report) -> u64 {
    let cs = u64::from(geo.cluster_size);
    let margin = (geo.clusters / 100).max((1 << 20) / cs);
    let needed = (report.held - report.bad + margin).max(fixed_end(geo));
    (needed * cs + u64::from(geo.sector_size)).next_multiple_of(1 << 20)
}

/// The end of what never moves: `$Boot` and the master file table's
/// first sixteen records, the system's, from where the boot sector says
/// it starts.
fn fixed_end(geo: &Geometry) -> u64 {
    let cs = u64::from(geo.cluster_size);
    let first_records = (16 * u64::from(geo.record_size)).div_ceil(cs);
    (geo.mft + first_records).max(BOOT_BYTES.div_ceil(cs))
}

/// The size a volume shrinks to.
#[derive(Clone, Copy, Debug)]
struct Target {
    /// Its sectors, as the boot sector counts them; the copy of the boot
    /// sector is in the next.
    sectors: u64,
    /// Its clusters.
    clusters: u64,
    /// The size of `$Bitmap`'s data at it, in bytes: a bit a cluster, in
    /// whole 8 bytes, as Windows makes it.
    bitmap_bytes: u64,
    /// The clusters that data takes.
    bitmap_clusters: u64,
}

impl Target {
    /// The size of a volume of `geo` that fills a partition of `bytes`
    /// bytes.
    fn new(geo: &Geometry, bytes: u64) -> Target {
        let (ss, cs) = (u64::from(geo.sector_size), u64::from(geo.cluster_size));
        let sectors = (bytes / ss).saturating_sub(1);
        let clusters = sectors / (cs / ss);
        let bitmap_bytes = clusters.div_ceil(8).next_multiple_of(8);
        Target { sectors, clusters, bitmap_bytes, bitmap_clusters: bitmap_bytes.div_ceil(cs) }
    }

    /// The partition it fills, in bytes.
    fn bytes(&self, geo: &Geometry) -> u64 {
        (self.sectors + 1) * u64::from(geo.sector_size)
    }
}

/// What lies past the new end, as the check's walk finds it: the runs a
/// shrink moves, and the records that say where they are.
struct Past {
    target: Target,
    /// The records with runs past the end, in order.
    records: Vec<u64>,
    /// The runs past the end, in spans.
    spans: Vec<Span>,
    /// Where the last span ends: its record, its attribute's number there
    /// and the VCN after it, which the next run continues if it starts
    /// there.
    open: Option<(u64, u16, u64)>,
}

/// Runs of one attribute past the end, one after another by VCN though
/// apart on the disk. A span is moved to one free run where there is one
/// long enough, and becomes a single run, however many it was: so a
/// file's run list shrinks as it moves, or stays as it was, rather than
/// outgrow its record.
struct Span {
    /// Its runs, `(first, count)`, in VCN order.
    runs: Vec<(u64, u64)>,
    /// Their clusters.
    len: u64,
    when: When,
}

impl Past {
    fn new(target: Target) -> Past {
        Past { target, records: Vec::new(), spans: Vec::new(), open: None }
    }

    /// Notes the run `run` of the attribute `attr` of record `number`,
    /// if it lies past the end. The walk gives an attribute's runs in
    /// VCN order.
    fn see(&mut self, number: u64, attr: &Attr<'_>, run: &Run) {
        // Bad clusters past the end are left out of the volume, not
        // moved.
        if is_bad_clusters(number, attr) {
            return;
        }
        let from = run.lcn.max(self.target.clusters);
        if from >= run.lcn + run.len {
            return;
        }
        let vcn = run.vcn + (from - run.lcn);
        let len = run.lcn + run.len - from;
        let when = if number == records::MFT {
            When::Table
        } else if is_bitmap_data(number, attr.kind(), attr.unnamed()) {
            When::Bitmap
        } else {
            When::Files
        };
        match self.spans.last_mut() {
            Some(span) if self.open == Some((number, attr.id(), vcn)) => {
                span.runs.push((from, len));
                span.len += len;
            }
            _ => self.spans.push(Span { runs: vec![(from, len)], len, when }),
        }
        self.open = Some((number, attr.id(), vcn + len));
        if self.records.last() != Some(&number) {
            self.records.push(number);
        }
    }
}

/// Whether an attribute of type `kind` of record `number`, named if not
/// `unnamed`, is `$Bitmap`'s data.
fn is_bitmap_data(number: u64, kind: u32, unnamed: bool) -> bool {
    number == records::BITMAP && kind == kind::DATA && unnamed
}

/// When a run's clusters are copied (the steps of the module
/// documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum When {
    /// Step 3: a file's.
    Files,
    /// Step 5: the master file table's, once the records are written.
    Table,
    /// Step 7: `$Bitmap`'s, once no other bit changes.
    Bitmap,
}

/// `len` clusters moved from `from` to `to`.
#[derive(Clone, Copy, Debug)]
struct Move {
    from: u64,
    len: u64,
    to: u64,
    when: When,
}

/// A shrink, worked out.
struct Plan {
    target: Target,
    /// The volume's clusters before.
    old_clusters: u64,
    /// The moves, in the order of where the clusters are now.
    moves: Vec<Move>,
    /// The records to rewrite, in order.
    records: Vec<u64>,
}

impl Plan {
    /// Works out where the runs `past` found go in the volume `vol`,
    /// whose `$Bitmap` is in `bitmap`, and checks that each record holds
    /// its runs once moved. Fails as [`Refused::TooSmall`] with
    /// `smallest` if the free clusters below the end are too few.
    fn make(
        vol: &Volume,
        bitmap: &Runs,
        past: Past,
        smallest: u64,
    ) -> core::result::Result<Plan, Refused> {
        let mut free = free_below(vol, bitmap, past.target.clusters)?;
        let mut moves = Vec::new();
        place(&mut free, &past.spans, &mut moves).ok_or(Refused::TooSmall(smallest))?;
        moves.sort_by_key(|m| m.from);
        let plan = Plan {
            target: past.target,
            old_clusters: vol.geo.clusters,
            moves,
            records: past.records,
        };
        let cs = u64::from(vol.geo.cluster_size);
        let mut buf = zeroed(vol.geo.record_size as usize)?;
        let mut check = |number: u64| -> core::result::Result<(), Refused> {
            vol.read_record(number, &mut buf)?;
            match plan.rewrite(&mut buf) {
                Err(Error::NoSpace) => Err(Refused::Fragmented(number)),
                other => Ok(other?),
            }
        };
        for &number in &plan.records {
            check(number)?;
        }
        // `$Bitmap`'s record and `$BadClus`'s change whatever moves, as
        // their data is cut to the new size.
        vol.read_record(records::BITMAP, &mut buf)?;
        plan.rewrite(&mut buf)?;
        plan.cut_bitmap_record(&mut buf, cs).map_err(|err| match err {
            Error::NoSpace => Refused::Fragmented(records::BITMAP),
            other => Refused::Failed(other),
        })?;
        vol.read_record(records::BAD_CLUSTERS, &mut buf)?;
        plan.cut_bad_clusters(&mut buf, cs)?;
        Ok(plan)
    }

    /// Where the clusters `lcn..lcn + len` are once moved: runs
    /// `(first, count)`.
    fn remap(&self, lcn: u64, len: u64) -> Result<Vec<(u64, u64)>> {
        let end = lcn + len;
        let mut out = Vec::new();
        let mut at = lcn;
        while at < end {
            if at < self.target.clusters {
                let n = end.min(self.target.clusters) - at;
                out.push((at, n));
                at += n;
                continue;
            }
            let i = self.moves.partition_point(|m| m.from + m.len <= at);
            let m = self.moves.get(i).filter(|m| m.from <= at).ok_or(Error::Corrupt)?;
            let n = end.min(m.from + m.len) - at;
            out.push((m.to + (at - m.from), n));
            at += n;
        }
        Ok(out)
    }

    /// Rewrites the run lists of the non-resident attributes of the
    /// record `buf` to say where their clusters are once moved. Fails
    /// with [`Error::NoSpace`] if the record has no room for them.
    fn rewrite(&self, buf: &mut [u8]) -> Result<()> {
        Record::new(buf)?;
        let mut at = usize::from(u16_at(buf, 0x14));
        while at + 8 <= edit::used(buf) && u32_at(buf, at) != kind::END {
            if buf[at + 8] != 0 {
                self.rewrite_attr(buf, at)?;
            }
            let len = u32_at(buf, at + 4) as usize;
            if len < 0x18 {
                return Err(Error::Corrupt);
            }
            at += len;
        }
        Ok(())
    }

    /// [`Plan::rewrite`] for the non-resident attribute at `at`.
    fn rewrite_attr(&self, buf: &mut [u8], at: usize) -> Result<()> {
        let lowest = u64_at(buf, at + 0x10);
        let highest = u64_at(buf, at + 0x18);
        let mut runs = attr_runs(buf, at, self.old_clusters)?;
        let end = self.target.clusters;
        let mut swaps = Vec::new();
        for run in runs.as_slice() {
            if run.lcn == HOLE || run.lcn + run.len <= end {
                continue;
            }
            let skip = end.saturating_sub(run.lcn);
            swaps.push((
                run.vcn + skip,
                run.len - skip,
                self.remap(run.lcn + skip, run.len - skip)?,
            ));
        }
        if swaps.is_empty() {
            return Ok(());
        }
        for (vcn, len, with) in &swaps {
            runs.replace(*vcn, *len, with)?;
        }
        set_runs(buf, at, &runs, lowest, highest)
    }

    /// Cuts `$Bitmap`'s data, in its record `buf`, to the new size, once
    /// moved below the end; returns the runs cut off, whose clusters are
    /// free.
    fn cut_bitmap_record(&self, buf: &mut [u8], cs: u64) -> Result<KVec<Run>> {
        Record::new(buf)?;
        let at = edit::find(buf, kind::DATA, &[]).ok_or(Error::Corrupt)?;
        // In one piece, Windows' and ntfs-3g's alike.
        if buf[at + 8] == 0 || u64_at(buf, at + 0x10) != 0 {
            return Err(Error::NotSupported);
        }
        let target = self.target;
        let mut runs = attr_runs(buf, at, self.old_clusters)?;
        if runs.clusters() < target.bitmap_clusters {
            return Err(Error::Corrupt);
        }
        let cut = runs.truncate(target.bitmap_clusters)?;
        set_runs(buf, at, &runs, 0, target.bitmap_clusters - 1)?;
        set_sizes(buf, at, target.bitmap_clusters * cs, target.bitmap_bytes, target.bitmap_bytes);
        Ok(cut)
    }

    /// Cuts `$BadClus`'s `$Bad`, in its record `buf`, to the new size: a
    /// hole as long as the volume, but for the bad clusters on it.
    fn cut_bad_clusters(&self, buf: &mut [u8], cs: u64) -> Result<()> {
        Record::new(buf)?;
        let at = edit::find(buf, kind::DATA, &BAD).ok_or(Error::NotSupported)?;
        // In one piece, in `$BadClus`'s own record.
        if buf[at + 8] == 0 || u64_at(buf, at + 0x10) != 0 {
            return Err(Error::NotSupported);
        }
        let clusters = self.target.clusters;
        let mut runs = attr_runs(buf, at, self.old_clusters)?;
        runs.truncate(clusters)?;
        runs.append(HOLE, clusters.saturating_sub(runs.clusters()))?;
        set_runs(buf, at, &runs, 0, clusters - 1)?;
        let bytes = clusters * cs;
        set_sizes(buf, at, bytes, bytes, u64_at(buf, at + 0x38).min(bytes));
        if u16_at(buf, at + 0x0c) & (attr_flags::COMPRESSED | attr_flags::SPARSE) != 0 {
            put_u64(buf, at + 0x40, runs.allocated_in(0, clusters) * cs);
        }
        Ok(())
    }

    /// Shrinks the volume of `inner` as worked out, in the steps of the
    /// module documentation, telling `progress` how far the copies are.
    fn carry_out(&self, inner: &Inner, progress: &mut dyn FnMut(Step)) -> Result<()> {
        let vol = &inner.vol;
        let cache = &vol.cache;
        let device = cache.device();
        let geo = vol.geo;
        let cs = u64::from(geo.cluster_size);
        // 1. Marked dirty, to stay so; the journal emptied.
        inner.keep_dirty.store(true, Ordering::Relaxed);
        inner.changing()?;
        // 2. The clusters the copies go to are taken.
        self.mark(inner, true, |m| Some(m.to))?;
        // 3. The files' clusters, copied from the disk as the cache last
        // left it.
        cache.sync()?;
        let mut copier = Copier::new(vol, self.moves.iter().map(|m| m.len).sum::<u64>() * cs)?;
        self.copy(&mut copier, When::Files, progress)?;
        device.flush()?;
        // 4. Each record but the master file table's and `$Bitmap`'s says
        // where its copies are.
        let mut mirror = geo.mft_mirror;
        let mut buf = zeroed(geo.record_size as usize)?;
        for &number in &self.records {
            if number == records::MFT || number == records::BITMAP {
                continue;
            }
            vol.read_record(number, &mut buf)?;
            self.rewrite(&mut buf)?;
            // `$MFTMirr`'s copies are written to where it is now, its own
            // record's among them.
            if number == records::MFT_MIRROR {
                let runs = named_runs(&buf, kind::DATA, self.old_clusters)?;
                mirror = runs.as_slice().first().map_or(mirror, |run| run.lcn);
                vol.move_mirror(runs);
            }
            vol.write_record(number, &buf, false)?;
        }
        cache.sync()?;
        // 5. The master file table, then its first record.
        if self.moves.iter().any(|m| m.when == When::Table) {
            self.copy(&mut copier, When::Table, progress)?;
            device.flush()?;
            vol.read_record(records::MFT, &mut buf)?;
            self.rewrite(&mut buf)?;
            vol.move_mft(named_runs(&buf, kind::DATA, self.old_clusters)?);
            if let Some(bitmap) = inner.space.lock().records.as_mut() {
                bitmap.runs = named_runs(&buf, kind::BITMAP, self.old_clusters)?;
            }
            vol.write_record(records::MFT, &buf, true)?;
        }
        // 6. The clusters copied from are free, but `$Bitmap`'s, which it
        // is read from until step 7.
        self.mark(inner, false, |m| (m.when != When::Bitmap).then_some(m.from))?;
        cache.sync()?;
        // 7. `$Bitmap`'s clusters past the end.
        if self.moves.iter().any(|m| m.when == When::Bitmap) {
            self.move_bitmap(inner, &mut copier, progress)?;
        }
        // 8. The boot sector says the new size, and where `$MFTMirr` is.
        let mut boot = zeroed(geo.sector_size as usize)?;
        cache.read(0, &mut boot)?;
        put_u64(&mut boot, 0x28, self.target.sectors);
        put_u64(&mut boot, 0x38, mirror);
        cache.write_through(0, &boot)?;
        device.flush()?;
        // 9. `$Bitmap` and `$BadClus` are cut to it.
        self.cut_bitmap(inner)?;
        vol.read_record(records::BAD_CLUSTERS, &mut buf)?;
        self.cut_bad_clusters(&mut buf, cs)?;
        vol.write_record(records::BAD_CLUSTERS, &buf, true)?;
        cache.sync()?;
        // 10. The copy of the boot sector, in the new last sector.
        cache.write_through(self.target.sectors * u64::from(geo.sector_size), &boot)?;
        Ok(device.flush()?)
    }

    /// Marks in `$Bitmap` the clusters `place` says of each move, `used`
    /// or free; none for a move it gives none.
    fn mark(&self, inner: &Inner, used: bool, place: impl Fn(&Move) -> Option<u64>) -> Result<()> {
        let mut space = inner.space.lock();
        {
            let bits = space.clusters.bits(&inner.vol.cache, &inner.vol.geo);
            for m in &self.moves {
                if let Some(first) = place(m) {
                    bits.set(first, m.len, used)?;
                }
            }
        }
        space.clusters.free = None;
        Ok(())
    }

    /// Copies the runs moved at `when` with `copier`.
    fn copy(
        &self,
        copier: &mut Copier<'_>,
        when: When,
        progress: &mut dyn FnMut(Step),
    ) -> Result<()> {
        for m in self.moves.iter().filter(|m| m.when == when) {
            copier.copy(m, progress)?;
        }
        Ok(())
    }

    /// Moves `$Bitmap`'s clusters past the end below it (step 7): copied
    /// as every other bit is now, then its record says where, and its
    /// clusters copied from are marked free in it where it is now.
    fn move_bitmap(
        &self,
        inner: &Inner,
        copier: &mut Copier<'_>,
        progress: &mut dyn FnMut(Step),
    ) -> Result<()> {
        let vol = &inner.vol;
        let mut space = inner.space.lock();
        self.copy(copier, When::Bitmap, progress)?;
        vol.cache.device().flush()?;
        let mut buf = zeroed(vol.geo.record_size as usize)?;
        vol.read_record(records::BITMAP, &mut buf)?;
        self.rewrite(&mut buf)?;
        vol.write_record(records::BITMAP, &buf, true)?;
        space.clusters.bitmap = named_runs(&buf, kind::DATA, self.old_clusters)?;
        let bits = space.clusters.bits(&vol.cache, &vol.geo);
        for m in self.moves.iter().filter(|m| m.when == When::Bitmap) {
            bits.set(m.from, m.len, false)?;
        }
        drop(space);
        Ok(vol.cache.sync()?)
    }

    /// Cuts `$Bitmap` to the new size (step 9): the bits of its last
    /// bytes past the last cluster are set, as Windows sets them; its
    /// record says its new size; and its clusters past it are given back.
    fn cut_bitmap(&self, inner: &Inner) -> Result<()> {
        let vol = &inner.vol;
        let geo = vol.geo;
        let target = self.target;
        let mut space = inner.space.lock();
        {
            let len = target.bitmap_bytes * 8;
            let bits = Bits { cache: &vol.cache, geo: &geo, runs: &space.clusters.bitmap, len };
            bits.set(target.clusters, len - target.clusters, true)?;
        }
        let mut buf = zeroed(geo.record_size as usize)?;
        vol.read_record(records::BITMAP, &mut buf)?;
        let cut = self.cut_bitmap_record(&mut buf, u64::from(geo.cluster_size))?;
        vol.write_record(records::BITMAP, &buf, true)?;
        let mut runs = space.clusters.bitmap.copy()?;
        runs.truncate(target.bitmap_clusters)?;
        space.clusters.bitmap = runs;
        space.clusters.free = None;
        let len = target.clusters;
        let bits = Bits { cache: &vol.cache, geo: &geo, runs: &space.clusters.bitmap, len };
        for run in &cut {
            let end = (run.lcn + run.len).min(target.clusters);
            if run.lcn < end {
                bits.set(run.lcn, end - run.lcn, false)?;
            }
        }
        Ok(())
    }
}

/// The free runs below cluster `end` of `vol`, whose `$Bitmap` is in
/// `bitmap`: `(count, first)`, so that they sort by length.
fn free_below(vol: &Volume, bitmap: &Runs, end: u64) -> Result<BTreeSet<(u64, u64)>> {
    let bits = Bits { cache: &vol.cache, geo: &vol.geo, runs: bitmap, len: vol.geo.clusters };
    let mut free = BTreeSet::new();
    let mut at = 0;
    while let Some((first, count)) = bits.find_clear(at, end, u64::MAX)? {
        free.insert((count, first));
        at = first + count;
    }
    Ok(free)
}

/// Finds room in `free` (free runs, `(count, first)`) for the spans
/// `spans`: the master file table's first, so that it moves in as few
/// runs as can be, as its first record holds them all; then the longest
/// first. Each goes in the shortest free run it fits in, or else in the
/// longest there are, in pieces. Adds the moves to `moves`. None if
/// `free` has too little room.
fn place(free: &mut BTreeSet<(u64, u64)>, spans: &[Span], moves: &mut Vec<Move>) -> Option<()> {
    let mut order: Vec<&Span> = spans.iter().collect();
    order.sort_by_key(|span| (span.when != When::Table, Reverse(span.len)));
    for span in order {
        let mut places = Vec::new();
        let mut left = span.len;
        while left > 0 {
            let fits = free.range((left, 0)..).next().copied();
            let (count, first) = fits.or_else(|| free.last().copied())?;
            free.remove(&(count, first));
            let n = count.min(left);
            if count > n {
                free.insert((count - n, first + n));
            }
            places.push((first, n));
            left -= n;
        }
        // The span's runs, laid one after another along those places.
        let mut places = places.into_iter();
        let (mut to, mut room) = places.next()?;
        for &(mut from, mut len) in &span.runs {
            while len > 0 {
                if room == 0 {
                    (to, room) = places.next()?;
                }
                let n = len.min(room);
                moves.push(Move { from, len: n, to, when: span.when });
                (from, len, to, room) = (from + n, len - n, to + n, room - n);
            }
        }
    }
    Some(())
}

/// The runs of the non-resident attribute at `at` of the record `buf`,
/// on a volume of `clusters` clusters, from its first VCN: a hole before
/// a piece that starts later.
pub(crate) fn attr_runs(buf: &[u8], at: usize, clusters: u64) -> Result<Runs> {
    let len = u32_at(buf, at + 4) as usize;
    let runs_at = usize::from(u16_at(buf, at + 0x20));
    let lowest = u64_at(buf, at + 0x10);
    let bytes = buf.get(at + runs_at..at + len).ok_or(Error::Corrupt)?;
    let mut runs = Runs::new();
    runs.append(HOLE, lowest)?;
    runs.add(bytes, lowest, u64_at(buf, at + 0x18), clusters)?;
    Ok(runs)
}

/// The runs of the unnamed non-resident attribute of type `kind` of the
/// record `buf`, in one piece there.
fn named_runs(buf: &[u8], kind: u32, clusters: u64) -> Result<Runs> {
    let at = edit::find(buf, kind, &[]).ok_or(Error::Corrupt)?;
    if buf[at + 8] == 0 || u64_at(buf, at + 0x10) != 0 {
        return Err(Error::NotSupported);
    }
    attr_runs(buf, at, clusters)
}

/// Puts `runs`, from VCN `lowest` to `highest`, as the run list of the
/// non-resident attribute at `at` of the record `buf`, resizing it. Fails
/// with [`Error::NoSpace`] if the record has no room.
pub(crate) fn set_runs(
    buf: &mut [u8],
    at: usize,
    runs: &Runs,
    lowest: u64,
    highest: u64,
) -> Result<()> {
    let packed = runs.pack(lowest, highest.wrapping_add(1))?;
    let runs_at = usize::from(u16_at(buf, at + 0x20));
    let len = (runs_at + packed.len()).next_multiple_of(8);
    edit::resize(buf, at, len)?;
    buf[at + runs_at..at + runs_at + packed.len()].copy_from_slice(&packed);
    buf[at + runs_at + packed.len()..at + len].fill(0);
    put_u64(buf, at + 0x18, highest);
    Ok(())
}

/// Sets the sizes of the non-resident attribute at `at` of `buf`.
fn set_sizes(buf: &mut [u8], at: usize, allocated: u64, size: u64, initialized: u64) {
    put_u64(buf, at + 0x28, allocated);
    put_u64(buf, at + 0x30, size);
    put_u64(buf, at + 0x38, initialized);
}

/// Copies clusters on the device itself, around a megabyte at a time,
/// counting what it copied.
struct Copier<'a> {
    vol: &'a Volume,
    pages: Vec<CachePage>,
    done: u64,
    total: u64,
}

impl<'a> Copier<'a> {
    /// A copier for `vol`, of `total` bytes in all.
    fn new(vol: &'a Volume, total: u64) -> Result<Copier<'a>> {
        let count = vol.cache.device().info().max_segments.clamp(1, 256);
        let mut pages = Vec::with_capacity(count);
        for _ in 0..count {
            pages.push(CachePage::new()?);
        }
        Ok(Copier { vol, pages, done: 0, total })
    }

    /// Copies the clusters of `m`. Whatever the cache holds of either
    /// place goes: it was synced first, so nothing it holds is lost.
    fn copy(&mut self, m: &Move, progress: &mut dyn FnMut(Step)) -> Result<()> {
        let geo = &self.vol.geo;
        let cache = &self.vol.cache;
        let device = cache.device();
        let bytes = m.len * u64::from(geo.cluster_size);
        let (mut from, mut to) = (geo.cluster_offset(m.from), geo.cluster_offset(m.to));
        cache.discard(from, bytes as usize);
        cache.discard(to, bytes as usize);
        let mut left = bytes;
        while left > 0 {
            let n = left.min((self.pages.len() * PAGE_SIZE) as u64) as usize;
            let used = n.div_ceil(PAGE_SIZE);
            let len = |i: usize| (n - i * PAGE_SIZE).min(PAGE_SIZE);
            let mut into: Vec<SegmentMut<'_>> = Vec::with_capacity(used);
            for (i, page) in self.pages[..used].iter_mut().enumerate() {
                into.push(SegmentMut { page, offset: 0, len: len(i) });
            }
            device.read(from / SECTOR, &mut into)?;
            drop(into);
            let mut out: Vec<Segment<'_>> = Vec::with_capacity(used);
            for (i, page) in self.pages[..used].iter().enumerate() {
                out.push(Segment { page, offset: 0, len: len(i) });
            }
            device.write(to / SECTOR, &out)?;
            (from, to, left) = (from + n as u64, to + n as u64, left - n as u64);
            self.done += n as u64;
            progress(Step::Moving { done: self.done, total: self.total });
        }
        Ok(())
    }
}
