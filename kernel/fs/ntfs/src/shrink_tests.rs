// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Host tests of shrinking a volume (ADR 0312). Volumes `mkntfs` made,
//! which puts `$MFTMirr` and the journal in their middle, are filled by
//! Radix past where they are to end, with gaps below it, and shrunk on a
//! RAM disk. Then ntfs-3g (`ntfsresize --info`, `ntfsfix -n`) and The
//! Sleuth Kit check what is left, Radix's own check finds it sound and
//! reads every file back, and in CI Windows' `chkdsk` checks it too
//! (`windows_tests`). Some volumes have `$Bitmap` and part of the master
//! file table moved near their end first, as Windows may leave them.
//! Power cuts all through a shrink leave every file readable.

use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;
use std::{format, vec};

use radix_block::ram::RamDisk;
use radix_block::{BlockDevice, DeviceRef};
use radix_vfs::mem::{self, KArc};
use radix_vfs::{FileSystem, Kind, Setting};

use crate::edit;
use crate::fs::records;
use crate::record::kind;
use crate::shrink::{self, Options, Refused, Shrunk, Step, attr_runs, set_runs};
use crate::tests::{Image, VOLUMES, pattern, read_all, walk};
use crate::windows_tests::{self, Found, Listed};
use crate::write_tests::{DIR, Volume, check_here, interleaved, make_file, ntfs_3g_check, set};

/// The sizes of the files that fill a volume.
const FILLING: [usize; 6] = [0, 600, 4097, 70_000, 300_001, 1 << 20];

/// A RAM disk of blocks of `block` bytes holding `image`.
fn disk_of(image: &[u8], block: u32) -> KArc<RamDisk> {
    let disk = mem::arc(RamDisk::with_blocks(image.len() as u64 / 512, false, block)).unwrap();
    disk.load(0, image).unwrap();
    disk
}

/// The bytes on `disk`.
fn bytes_of(disk: &RamDisk) -> Vec<u8> {
    let mut image = vec![0u8; (disk.info().sectors * 512) as usize];
    disk.dump(0, &mut image);
    image
}

/// Fills `volume`, of clusters of `cluster` bytes, until about seven
/// tenths of it is used: a directory of files of every size, two files
/// in many pieces, and a file with a stream; then every third file of
/// the directory goes, leaving gaps. Returns what is left: each path and
/// its bytes, a directory's none, and a stream's at `file:stream`.
fn fill(volume: &Volume, cluster: usize) -> BTreeMap<String, Vec<u8>> {
    let root = volume.root();
    let blocks = volume.fs.info().blocks;
    let mut kept = BTreeMap::new();
    let windows = root.create("Windows", Kind::Directory, DIR).unwrap();
    kept.insert(String::from("Windows"), Vec::new());
    let mut count = 0;
    while volume.fs.info().free > blocks / 2 {
        let data = pattern(count, FILLING[count as usize % FILLING.len()]);
        let name = format!("file {count}.bin");
        make_file(&windows, &name, &data, 1 << 16);
        kept.insert(format!("Windows/{name}"), data);
        count += 1;
    }
    let pieces = (blocks / 40).min(300) as usize;
    kept.extend(interleaved(&root, &["one.bin", "two.bin"], pieces, cluster));
    let notes = make_file(&root, "notes.txt", b"what Windows keeps", usize::MAX);
    let zone = b"[ZoneTransfer]\r\nZoneId=3\r\n";
    set(&notes, "user.Zone.Identifier", zone, Setting::Create);
    kept.insert(String::from("notes.txt"), b"what Windows keeps".to_vec());
    kept.insert(String::from("notes.txt:Zone.Identifier"), zone.to_vec());
    while volume.fs.info().free > blocks * 3 / 10 {
        let data = pattern(count, FILLING[count as usize % FILLING.len()]);
        let name = format!("file {count}.bin");
        make_file(&windows, &name, &data, 1 << 16);
        kept.insert(format!("Windows/{name}"), data);
        count += 1;
    }
    for gone in (0..count).step_by(3) {
        let name = format!("file {gone}.bin");
        windows.remove(&name).unwrap();
        kept.remove(&format!("Windows/{name}"));
    }
    kept
}

/// Shrinks the volume `image`, on a disk of blocks of `block` bytes, to
/// `bytes`, asking that Windows check it at its next start if
/// `windows_checks`; returns the disk after, what the shrink did, and
/// the steps it went through, a run of copies as one.
fn shrink_on(
    image: &[u8],
    block: u32,
    bytes: u64,
    windows_checks: bool,
) -> (KArc<RamDisk>, Result<Shrunk, Refused>, Vec<Step>) {
    let disk = disk_of(image, block);
    let device: DeviceRef = disk.clone();
    let options = Options { bytes, dry_run: false, check_at_windows_start: windows_checks };
    let mut steps = Vec::new();
    let result = shrink::shrink(device, options, &mut |step| {
        let copying = matches!(step, Step::Moving { .. });
        if !(copying && matches!(steps.last(), Some(Step::Moving { .. }))) {
            steps.push(step);
        }
    });
    (disk, result, steps)
}

/// The end of the last cluster in use on the volume `image`, on a disk
/// of blocks of `block` bytes, in bytes.
fn last_used(image: &[u8], block: u32) -> u64 {
    let volume = Volume::mount_in(block, image, true).unwrap();
    let inner = &volume.fs.inner;
    let geo = inner.vol.geo;
    let space = inner.space.lock();
    let mut bitmap = vec![0u8; geo.clusters.div_ceil(8) as usize];
    crate::space::read_runs(&inner.vol.cache, &geo, &space.clusters.bitmap, 0, &mut bitmap)
        .unwrap();
    let last = (0..geo.clusters).rev().find(|&c| bitmap[(c / 8) as usize] & 1 << (c % 8) != 0);
    (last.unwrap() + 1) * u64::from(geo.cluster_size)
}

/// What `shrink::info` says of `image`, on a disk of blocks of `block`
/// bytes.
fn info_of(image: &[u8], block: u32) -> shrink::Info {
    let device: DeviceRef = disk_of(image, block);
    shrink::info(device).unwrap()
}

/// Moves the clusters of the unnamed attribute of type `kind` of record
/// `number`, from its VCN `from` on, to the free clusters from `to` on:
/// a system file where Windows may have put it, which `mkntfs` does not.
/// Returns the cluster after the last it moved them to.
fn relocate(volume: &Volume, number: u64, kind: u32, from: u64, to: u64) -> u64 {
    let inner = &volume.fs.inner;
    let vol = &inner.vol;
    let geo = vol.geo;
    volume.fs.sync().unwrap();
    let mut buf = vec![0u8; geo.record_size as usize];
    vol.read_record(number, &mut buf).unwrap();
    let at = edit::find(&buf, kind, &[]).unwrap();
    let mut runs = attr_runs(&buf, at, geo.clusters).unwrap();
    let len = runs.clusters() - from;
    let mut space = inner.space.lock();
    let bits = space.clusters.bits(&vol.cache, &geo);
    assert_eq!(bits.clear_from(to, len).unwrap(), len, "{len} clusters from {to} are not free");
    bits.set(to, len, true).unwrap();
    let mut cluster = vec![0u8; geo.cluster_size as usize];
    for vcn in from..from + len {
        let (lcn, _) = runs.map(vcn).unwrap();
        vol.cache.read(geo.cluster_offset(lcn), &mut cluster).unwrap();
        vol.cache.write(geo.cluster_offset(to + vcn - from), &cluster).unwrap();
    }
    vol.cache.sync().unwrap();
    let freed = runs.replace(from, len, &[(to, len)]).unwrap();
    let last = runs.clusters() - 1;
    set_runs(&mut buf, at, &runs, 0, last).unwrap();
    vol.write_record(number, &buf, true).unwrap();
    match (number, kind) {
        (records::MFT, kind::DATA) => vol.move_mft(runs),
        (records::MFT, kind::BITMAP) => space.records.as_mut().unwrap().runs = runs,
        (records::BITMAP, kind::DATA) => space.clusters.bitmap = runs,
        _ => panic!("only the master file table's attributes and $Bitmap's data are moved"),
    }
    let bits = space.clusters.bits(&vol.cache, &geo);
    for run in &freed {
        bits.set(run.lcn, run.len, false).unwrap();
    }
    space.clusters.free = None;
    drop(space);
    volume.fs.sync().unwrap();
    to + len
}

/// Checks the shrunk volume on `disk`, which `shrunk` says is
/// `shrunk.bytes` long now: the check after the shrink found it sound,
/// and the volume cut to its new size is sound to ntfs-3g, The Sleuth
/// Kit and Radix, holding `kept`, and kept for Windows' `chkdsk` if
/// `windows`. Radix writes to it after, and ntfs-3g finds that sound
/// too. Returns it.
fn check_shrunk(
    disk: &RamDisk,
    shrunk: &Shrunk,
    block: u32,
    kept: &BTreeMap<String, Vec<u8>>,
    windows: bool,
) -> Vec<u8> {
    let report = &shrunk.report;
    assert!(report.sound() && report.lost == 0, "the check after the shrink: {report:?}");
    let image = bytes_of(disk)[..shrunk.bytes as usize].to_vec();
    // The copy of the boot sector is in the last sector.
    let sector = u16::from_le_bytes([image[11], image[12]]) as usize;
    assert!(image[image.len() - sector..] == image[..sector], "the boot sector's copy");
    let find = check_here(&image, block, kept);
    if windows {
        windows_tests::keep(&image, &find, Listed::All);
    }
    let after = info_of(&image, block);
    assert!(after.report.sound() && after.report.lost == 0, "{:?}", after.report);
    assert_eq!(after.bytes, shrunk.bytes);
    // Radix writes to the shrunk volume as to any.
    let volume = Volume::new_in(block, &image);
    let data = pattern(77, 50_000);
    make_file(&volume.root(), "after.bin", &data, 4096);
    let written = volume.image();
    drop(volume);
    ntfs_3g_check(&written);
    let volume = Volume::mount_in(block, &written, true).unwrap();
    assert!(read_all(&walk(&volume.root(), "after.bin").unwrap()) == data);
    image
}

#[test]
fn a_shrunk_volume_keeps_every_file() {
    for (mib, sector, cluster) in VOLUMES {
        // Room for two shrinks, past the margin of a MiB.
        let image = Image::mkntfs(mib.max(32), sector, cluster);
        let volume = Volume::new_in(sector, &image.bytes());
        let kept = fill(&volume, cluster as usize);
        let full = volume.image();
        drop(volume);
        let info = info_of(&full, sector);
        assert!(info.report.sound() && info.report.lost == 0, "{:?}", info.report);
        assert!(info.kept.is_none() && !info.hibernated && !info.dirty);
        // Halfway from the smallest to the end of the last file, with the
        // master file table's mirror and the journal, in the middle,
        // moved below the end on the larger volumes.
        let halfway = ((info.smallest + last_used(&full, sector)) / 2) >> 20 << 20;
        assert!(halfway > info.smallest, "{halfway}, {}", info.smallest);
        let (disk, shrunk, steps) = shrink_on(&full, sector, halfway, false);
        let shrunk = shrunk.unwrap();
        assert_eq!(shrunk.bytes, halfway);
        assert!(shrunk.moved > 0 && shrunk.records > 0, "{shrunk:?}");
        assert_eq!(steps.len(), 3, "{steps:?}");
        let image = check_shrunk(&disk, &shrunk, sector, &kept, false);
        // Then to the smallest it goes to.
        let smallest = info_of(&image, sector).smallest;
        let (disk, again, _) = shrink_on(&image, sector, smallest, false);
        check_shrunk(&disk, &again.unwrap(), sector, &kept, true);
    }
}

#[test]
fn the_system_files_windows_moves_move_too() {
    for (mib, sector, cluster) in [(16, 512, 4096), (16, 512, 512)] {
        let image = Image::mkntfs(mib, sector, cluster);
        let volume = Volume::new_in(sector, &image.bytes());
        let kept = fill(&volume, cluster as usize);
        let geo = *volume.fs.geometry();
        // `$Bitmap`, and the master file table's records past its first
        // sixteen and its bitmap of them, near the end, where the files
        // past it are.
        let near_end = geo.clusters - geo.clusters / 20;
        let first = (16 * u64::from(geo.record_size)).div_ceil(u64::from(geo.cluster_size));
        let next = relocate(&volume, records::BITMAP, kind::DATA, 0, near_end);
        let next = relocate(&volume, records::MFT, kind::DATA, first, next + 3);
        relocate(&volume, records::MFT, kind::BITMAP, 0, next + 5);
        let full = volume.image();
        drop(volume);
        check_here(&full, sector, &kept);
        let info = info_of(&full, sector);
        let (disk, shrunk, _) = shrink_on(&full, sector, info.smallest, false);
        check_shrunk(&disk, &shrunk.unwrap(), sector, &kept, true);
    }
}

#[test]
fn windows_checks_a_shrunk_volume_at_its_next_start() {
    let image = Image::mkntfs(16, 512, 4096);
    let volume = Volume::new_in(512, &image.bytes());
    let kept = fill(&volume, 4096);
    let full = volume.image();
    drop(volume);
    let info = info_of(&full, 512);
    let (disk, shrunk, _) = shrink_on(&full, 512, info.smallest, true);
    let shrunk = shrunk.unwrap();
    assert!(shrunk.report.sound() && shrunk.report.lost == 0);
    let image = bytes_of(&disk)[..shrunk.bytes as usize].to_vec();
    // Marked dirty: Radix mounts it to read until Windows checks it.
    let after = info_of(&image, 512);
    assert!(after.dirty && after.kept.is_some());
    let volume = Volume::mount_in(512, &image, true).unwrap();
    for (path, data) in kept.iter().filter(|(path, _)| !path.contains(':')) {
        let node = walk(&volume.root(), path).unwrap();
        if node.stat().kind == Kind::File {
            assert!(read_all(&node) == *data, "{path}");
        }
    }
    let find: Vec<(&str, Found<'_>)> = kept
        .iter()
        .map(|(path, data)| match (path.contains(':'), path.as_str()) {
            (true, _) => (path.as_str(), Found::Stream(data)),
            (false, "Windows") => (path.as_str(), Found::Dir),
            (false, _) => (path.as_str(), Found::File(data)),
        })
        .collect();
    windows_tests::keep(&image, &find, Listed::Cut);
}

#[test]
fn a_shrink_it_refuses_changes_nothing() {
    let image = Image::mkntfs(64, 512, 4096);
    let volume = Volume::new_in(512, &image.bytes());
    fill(&volume, 4096);
    let full = volume.image();
    drop(volume);
    let info = info_of(&full, 512);
    let refuse = |image: &[u8], bytes: u64| {
        let (disk, result, _) = shrink_on(image, 512, bytes, false);
        assert!(bytes_of(&disk) == image, "a refused shrink wrote");
        result.unwrap_err()
    };
    // Too small, and not smaller.
    let Refused::TooSmall(smallest) = refuse(&full, info.smallest - (1 << 20)) else { panic!() };
    assert_eq!(smallest, info.smallest);
    assert!(matches!(refuse(&full, info.bytes), Refused::NotSmaller));
    // Nothing is written by a dry run.
    let disk = disk_of(&full, 512);
    let options = Options { bytes: info.smallest, dry_run: true, check_at_windows_start: true };
    let planned = shrink::shrink(disk.clone(), options, &mut |_| {}).unwrap();
    assert!(planned.moved > 0 && bytes_of(&disk) == full);
    // A cluster a file holds marked free: the check finds it.
    let mut unsound = full.clone();
    {
        let volume = Volume::mount_in(512, &full, false).unwrap();
        let inner = &volume.fs.inner;
        let geo = inner.vol.geo;
        let space = inner.space.lock();
        let lcn = attr_runs_of(&volume, records::UPCASE, kind::DATA).as_slice()[0].lcn;
        let at = space.clusters.bitmap.map(lcn / 8 / u64::from(geo.cluster_size)).unwrap().0;
        let byte = geo.cluster_offset(at) + lcn / 8 % u64::from(geo.cluster_size);
        unsound[byte as usize] &= !(1 << (lcn % 8));
    }
    let Refused::Unsound(report) = refuse(&unsound, info.smallest) else { panic!() };
    assert_eq!(report.count, 1, "{report:?}");
    assert!(report.problems[0].contains("marked free"), "{report:?}");
    // Marked dirty, or hibernated: Windows did not shut down fully.
    let dirty = {
        let volume = Volume::new_in(512, &full);
        volume.fs.inner.keep_dirty.store(true, core::sync::atomic::Ordering::Relaxed);
        make_file(&volume.root(), "x", b"x", 1);
        volume.image()
    };
    assert!(matches!(refuse(&dirty, info.smallest + (1 << 20)), Refused::Kept(_)));
    let hibernated = {
        let volume = Volume::new_in(512, &full);
        make_file(&volume.root(), "hiberfil.sys", b"HIBR and more", usize::MAX);
        volume.image()
    };
    let Refused::Kept(why) = refuse(&hibernated, info.smallest + (1 << 20)) else { panic!() };
    assert!(why.contains("Fast Startup"), "{why}");
}

/// The runs of the unnamed attribute of type `kind` of record `number`
/// of `volume`.
fn attr_runs_of(volume: &Volume, number: u64, kind: u32) -> crate::runs::Runs {
    let vol = &volume.fs.inner.vol;
    let mut buf = vec![0u8; vol.geo.record_size as usize];
    vol.read_record(number, &mut buf).unwrap();
    let at = edit::find(&buf, kind, &[]).unwrap();
    attr_runs(&buf, at, vol.geo.clusters).unwrap()
}

#[test]
fn a_file_in_many_pieces_moves_or_the_shrink_refuses_whole() {
    let image = Image::mkntfs(32, 512, 512);
    let volume = Volume::new_in(512, &image.bytes());
    let root = volume.root();
    // Two files of 2000 pieces, past the middle: their runs spread over
    // records. Then one goes, leaving 2000 gaps of a cluster.
    let filler = make_file(&root, "filler.bin", &pattern(1, 14 << 20), 1 << 16);
    let mut kept = interleaved(&root, &["one.bin", "two.bin"], 2000, 512);
    drop(filler);
    root.remove("filler.bin").unwrap();
    root.remove("two.bin").unwrap();
    kept.remove("two.bin");
    drop(root);
    let full = volume.image();
    drop(volume);
    let info = info_of(&full, 512);
    let (disk, result, _) = shrink_on(&full, 512, info.smallest, false);
    match result {
        Ok(shrunk) => {
            check_shrunk(&disk, &shrunk, 512, &kept, true);
        }
        Err(Refused::Fragmented(_)) => assert!(bytes_of(&disk) == full, "a refused shrink wrote"),
        Err(other) => panic!("{other}"),
    }
}

#[test]
fn a_power_cut_leaves_every_file_readable() {
    let image = Image::mkntfs(16, 512, 4096);
    let volume = Volume::new_in(512, &image.bytes());
    let kept = fill(&volume, 4096);
    let geo = *volume.fs.geometry();
    let near_end = geo.clusters - geo.clusters / 20;
    let first = (16 * u64::from(geo.record_size)).div_ceil(u64::from(geo.cluster_size));
    let next = relocate(&volume, records::BITMAP, kind::DATA, 0, near_end);
    relocate(&volume, records::MFT, kind::DATA, first, next + 3);
    let full = volume.image();
    drop(volume);
    let bytes = info_of(&full, 512).smallest;
    let (disk, whole, _) = shrink_on(&full, 512, bytes, false);
    whole.unwrap();
    let requests = disk.requests();
    let find: Vec<(&str, Found<'_>)> = kept
        .iter()
        .map(|(path, data)| match (path.contains(':'), path.as_str()) {
            (true, _) => (path.as_str(), Found::Stream(data)),
            (false, "Windows") => (path.as_str(), Found::Dir),
            (false, _) => (path.as_str(), Found::File(data)),
        })
        .collect();
    // Forty cuts through it, and each of its last dozen requests; every
    // twentieth kept for Windows.
    let mut cuts: Vec<u64> = (0..requests).step_by((requests / 40) as usize).collect();
    cuts.extend(requests - 12..requests);
    for (i, n) in cuts.into_iter().enumerate() {
        let disk = disk_of(&full, 512);
        disk.fail_after(n as u32);
        let device: DeviceRef = disk.clone();
        let options = Options { bytes, dry_run: false, check_at_windows_start: false };
        let result = shrink::shrink(device, options, &mut |_| {});
        assert!(matches!(result, Err(Refused::Failed(_))), "cut after {n} of {requests}");
        disk.fail_after(u32::MAX);
        let cut = bytes_of(&disk);
        if cut == full {
            continue;
        }
        // Radix reads every file where its record says, whatever size
        // the boot sector says now.
        let volume = Volume::mount_in(512, &cut, true)
            .unwrap_or_else(|err| panic!("cut after {n} of {requests}: {err:?}"));
        for (path, data) in &kept {
            let at = |what: &str| format!("cut after {n} of {requests}: {path}: {what}");
            if let Some((file, stream)) = path.split_once(':') {
                let node = walk(&volume.root(), file).unwrap();
                let value = node.get_attribute(format!("user.{stream}").as_bytes()).unwrap();
                assert!(value == *data, "{}", at("other bytes"));
                continue;
            }
            let node = walk(&volume.root(), path)
                .unwrap_or_else(|err| panic!("{}", at(&format!("{err:?}"))));
            if node.stat().kind == Kind::File {
                assert!(read_all(&node) == *data, "{}", at("other bytes"));
            }
        }
        drop(volume);
        if i % 20 == 10 {
            windows_tests::keep(&cut, &find, Listed::Cut);
        }
    }
}
