// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Checking where a volume's clusters are (ADR 0312), as `ntfsresize
//! --info` does before it resizes a volume and `chkdsk` does in its first
//! stage: each file's runs lie on the volume, no cluster is held by two
//! files, and each cluster a file holds is marked in use in `$Bitmap`.
//! Clusters marked in use that no file holds are counted as lost: a power
//! cut leaves them, `chkdsk` gives them back, and they cost no one's data.
//!
//! A shrink ([`crate::shrink`]) checks a volume this way before it moves
//! anything, and refuses one with a problem: moving a cluster two files
//! hold, or one that `$Bitmap` calls free, would harm a file. The same
//! walk tells it which runs lie past the new end. Then it checks the
//! shrunk volume again.
//!
//! The walk reads every record of the master file table, base and
//! extension records alike: each piece of a non-resident attribute is in
//! exactly one of them, so every run is seen once. It keeps a bit for
//! each cluster: 32 MiB for a volume of a terabyte in 4 KiB clusters.

use std::format;
use std::string::String;
use std::vec;
use std::vec::Vec;

use radix_block::DeviceRef;
use radix_vfs::mem::KVec;
use radix_vfs::{Error, Result};

use crate::NtfsFs;
use crate::fs::{Volume, records};
use crate::record::{Attr, Record, fix_up};
use crate::runs::{HOLE, Run, Runs};
use crate::space::read_runs;

/// The most problems a report lists in words; it counts the rest.
const MAX_LISTED: usize = 50;

/// The records read from the disk at a time.
const BATCH: u64 = 64;

/// The largest master file table bitmap read: one of 2^31 records.
const MAX_RECORD_BITMAP: u64 = 256 << 20;

/// What a check found.
#[derive(Debug, Default)]
pub struct Report {
    /// What is wrong, in words, the first [`MAX_LISTED`]; none on a
    /// sound volume.
    pub problems: Vec<String>,
    /// How many problems there are, those not listed too.
    pub count: u64,
    /// The records in use: the files' base records and extension records.
    pub records: u64,
    /// The clusters files hold, bad ones among them.
    pub held: u64,
    /// The clusters `$Bitmap` marks in use.
    pub marked: u64,
    /// Of those, the clusters no file holds: lost, which `chkdsk` gives
    /// back.
    pub lost: u64,
    /// The bad clusters, which `$BadClus` holds so that nothing uses them.
    pub bad: u64,
    /// The volume's clusters.
    pub clusters: u64,
    /// The size of a cluster, in bytes.
    pub cluster_size: u32,
}

impl Report {
    /// Whether nothing is wrong. Lost clusters are not wrong.
    pub fn sound(&self) -> bool {
        self.count == 0
    }

    fn problem(&mut self, text: String) {
        self.count += 1;
        if self.problems.len() < MAX_LISTED {
            self.problems.push(text);
        }
    }
}

/// Checks the volume on `device`, changing nothing.
pub fn check(device: DeviceRef) -> Result<Report> {
    let fs = NtfsFs::mount(device, true)?;
    let bitmap = fs.inner.space.lock().clusters.bitmap.copy()?;
    walk(&fs.inner.vol, &bitmap, &mut |_, _, _| {})
}

/// Checks the volume `vol`, whose `$Bitmap` is in the clusters `bitmap`,
/// calling `seen` with each run on the disk of each non-resident
/// attribute, and the number of the record it is in, as the walk finds
/// them, record by record.
pub(crate) fn walk(
    vol: &Volume,
    bitmap: &Runs,
    seen: &mut dyn FnMut(u64, &Attr<'_>, &Run),
) -> Result<Report> {
    let geo = vol.geo;
    let mut report =
        Report { clusters: geo.clusters, cluster_size: geo.cluster_size, ..Report::default() };
    let in_use = record_bitmap(vol)?;
    let mut held = vec![0u8; geo.clusters.div_ceil(8) as usize];
    let rs = geo.record_size as usize;
    let records = vol.records();
    let mft = vol.mft_runs()?;
    let mut batch = vec![0u8; rs * BATCH as usize];
    let mut first = 0;
    while first < records {
        let count = BATCH.min(records - first);
        let bytes = &mut batch[..rs * count as usize];
        read_runs(&vol.cache, &geo, &mft, first * rs as u64, bytes)?;
        for (i, record) in bytes.chunks_mut(rs).enumerate() {
            let number = first + i as u64;
            let marked =
                in_use.get((number / 8) as usize).is_some_and(|b| b & 1 << (number % 8) != 0);
            visit(number, record, marked, geo.clusters, &mut held, &mut report, seen);
        }
        first += count;
    }
    compare(vol, bitmap, &held, &mut report)?;
    Ok(report)
}

/// Looks at record `number`, as read from the disk into `buf`, which the
/// master file table's bitmap marks in use if `marked`, on a volume of
/// `clusters` clusters: marks the clusters its attributes hold in `held`.
fn visit(
    number: u64,
    buf: &mut [u8],
    marked: bool,
    clusters: u64,
    held: &mut [u8],
    report: &mut Report,
    seen: &mut dyn FnMut(u64, &Attr<'_>, &Run),
) {
    // A record no file has had yet is zeros, and one given back keeps
    // its bytes but not its in-use flag.
    let readable = fix_up(buf, b"FILE").is_ok();
    let record = if readable { Record::new(buf).ok() } else { None };
    let Some(record) = record else {
        if marked {
            report.problem(format!("record {number} is marked in use but cannot be read"));
        }
        return;
    };
    if !record.in_use() {
        return;
    }
    report.records += 1;
    if !marked {
        report.problem(format!("record {number} is a file's but marked free"));
    }
    for attr in record.attrs() {
        let Ok(attr) = attr else {
            report.problem(format!("record {number} has a damaged attribute"));
            return;
        };
        if !attr.non_resident() {
            continue;
        }
        // A piece's runs from its first VCN, as a hole before them.
        let lowest = attr.lowest_vcn();
        let mut runs = Runs::new();
        let added = runs
            .append(HOLE, lowest)
            .and_then(|()| runs.add(attr.runs(), lowest, attr.highest_vcn(), clusters));
        if added.is_err() {
            report.problem(format!(
                "record {number}: the runs of an attribute of type {:#x} are damaged or pass the \
                 volume's end",
                attr.kind()
            ));
            continue;
        }
        let bad = is_bad_clusters(number, &attr);
        for run in runs.as_slice().iter().filter(|run| run.lcn != HOLE) {
            let clashes = hold(held, run.lcn, run.len);
            if clashes > 0 {
                report.problem(format!(
                    "record {number}: {clashes} of the clusters from {} are another file's too",
                    run.lcn
                ));
            }
            report.held += run.len;
            if bad {
                report.bad += run.len;
            }
            seen(number, &attr, run);
        }
    }
}

/// Whether the attribute `attr` of record `number` is `$BadClus`'s
/// `$Bad`, whose runs are the bad clusters.
pub(crate) fn is_bad_clusters(number: u64, attr: &Attr<'_>) -> bool {
    number == records::BAD_CLUSTERS
        && attr.kind() == crate::record::kind::DATA
        && crate::record::name_is(attr.name(), "$Bad")
}

/// Marks the clusters `lcn..lcn + len` held in `held`, a bit for each;
/// returns how many of them were already.
fn hold(held: &mut [u8], lcn: u64, len: u64) -> u64 {
    let end = lcn + len;
    let mut clashes = 0;
    let mut at = lcn;
    while at < end {
        let byte = &mut held[(at / 8) as usize];
        if at.is_multiple_of(8) && end - at >= 8 {
            // Eight at once, as most runs are long.
            clashes += u64::from(byte.count_ones());
            *byte = 0xff;
            at += 8;
        } else {
            let bit = 1u8 << (at % 8);
            clashes += u64::from(*byte & bit != 0);
            *byte |= bit;
            at += 1;
        }
    }
    clashes
}

/// Compares the clusters files hold, `held`, with those `$Bitmap` (in the
/// clusters `bitmap`) marks in use: one held but marked free is a
/// problem, as the next file to grow may take it; one marked but not
/// held is lost.
fn compare(vol: &Volume, bitmap: &Runs, held: &[u8], report: &mut Report) -> Result<()> {
    let clusters = vol.geo.clusters;
    let mut chunk = [0u8; 4096];
    let (mut free_held, mut first_free_held) = (0u64, None);
    let mut at = 0;
    while at < held.len() {
        let n = chunk.len().min(held.len() - at);
        read_runs(&vol.cache, &vol.geo, bitmap, at as u64, &mut chunk[..n])?;
        for (i, &bits) in chunk[..n].iter().enumerate() {
            let byte = at + i;
            // The bits of the last byte past the last cluster count for
            // nothing: Windows sets them.
            let count = (clusters - byte as u64 * 8).min(8);
            let mask = if count == 8 { 0xff } else { (1u8 << count) - 1 };
            let (marked, owned) = (bits & mask, held[byte] & mask);
            report.marked += u64::from(marked.count_ones());
            report.lost += u64::from((marked & !owned).count_ones());
            let unmarked = owned & !marked;
            if unmarked != 0 {
                free_held += u64::from(unmarked.count_ones());
                first_free_held
                    .get_or_insert(byte as u64 * 8 + u64::from(unmarked.trailing_zeros()));
            }
        }
        at += n;
    }
    if let Some(first) = first_free_held {
        report.problem(format!(
            "{free_held} clusters files hold are marked free in $Bitmap, the first {first}"
        ));
    }
    Ok(())
}

/// The master file table's bitmap: a bit for each record, set when a
/// file has it.
fn record_bitmap(vol: &Volume) -> Result<KVec<u8>> {
    let loaded = vol.load(records::MFT)?;
    let value = loaded.own_bitmap.ok_or(Error::Corrupt)?;
    vol.read_whole(&value, MAX_RECORD_BITMAP)
}
