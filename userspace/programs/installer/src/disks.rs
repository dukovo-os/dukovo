// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The disks, as the kernel lists them in `/kernel/block` (radix-sysfs(5)):
//! the medium the live system started from, which holds the release (the
//! install stick's DUKOVO_LIVE partition, or the install DVD), the disk or
//! drive it is on, which is never installed onto, and the names and sizes
//! of disks and their partitions. A disk's partition table is read as
//! rdisk(8) reads it, with the same engine, so that the layouts are worked
//! out from what rdisk will find.

use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The GPT type of an install stick's DUKOVO_LIVE partition, "Dukovo
/// live" (ADR 0308), as `/kernel/block/<name>/parttype` gives it.
pub const DUKOVO_LIVE: &str = "5894c662-af90-4b47-baaf-2e3396443b25";

/// The label of the stick's DUKOVO_LIVE Btrfs, and of the install DVD's
/// ISO 9660 volume (ADR 0433).
const LIVE_LABEL: &str = "DUKOVO_LIVE";

/// Where a disc's ISO 9660 primary volume descriptor is, in bytes (the
/// 2,048-byte sector 16), how it starts (its type, 1, and `CD001`), and
/// where in it the volume's label is, padded with spaces (ECMA-119 8.4).
const ISO_PVD: u64 = 16 * 2048;
const ISO_PVD_LEN: usize = 2048;
const ISO_PVD_START: &[u8] = b"\x01CD001";
const ISO_VOLUME_ID: std::ops::Range<usize> = 40..72;

/// Where the kernel lists disks and partitions.
const BLOCK: &str = "/kernel/block";

/// How long a new partition may take to appear in `/devices` after its
/// table is written.
const APPEAR: Duration = Duration::from_secs(10);

/// What the kernel says of the disk or partition `name` in its file
/// `attribute`, without the newline; `None` if it has no such file.
fn attribute(name: &str, attribute: &str) -> Option<String> {
    let text = std::fs::read_to_string(Path::new(BLOCK).join(name).join(attribute)).ok()?;
    Some(text.trim_end().to_string())
}

/// The names of every disk and partition.
fn names() -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(BLOCK).map_err(|err| format!("reading {BLOCK}: {err}"))?;
    let mut names: Vec<String> =
        entries.filter_map(|entry| entry.ok()?.file_name().into_string().ok()).collect();
    names.sort();
    Ok(names)
}

/// Whether `name` is a partition, not a whole disk.
#[must_use]
pub fn is_partition(name: &str) -> bool {
    attribute(name, "partition").is_some()
}

/// The size of the disk or partition `name`, in bytes.
///
/// # Errors
///
/// If the kernel has no such disk.
pub fn bytes(name: &str) -> Result<u64, String> {
    let sectors = attribute(name, "size").and_then(|text| text.parse::<u64>().ok());
    sectors.map(|s| s * 512).ok_or_else(|| format!("there is no disk {name}: lsblk lists them"))
}

/// The name of partition `number` of the disk `disk`, as the kernel names
/// it: `vda3`, or `nvme0n1p3` for a disk whose name ends in a digit.
#[must_use]
pub fn partition(disk: &str, number: u32) -> String {
    if disk.ends_with(|c: char| c.is_ascii_digit()) {
        format!("{disk}p{number}")
    } else {
        format!("{disk}{number}")
    }
}

/// Whether `name` is an optical drive, which reads discs and is never a
/// disk to install onto: the kernel gives it a `disc` file.
#[must_use]
pub fn is_disc(name: &str) -> bool {
    attribute(name, "disc").is_some()
}

/// The disk the partition `name` is on.
#[must_use]
pub fn disk_of(name: &str) -> String {
    let digits = name.trim_end_matches(|c: char| c.is_ascii_digit());
    match digits.strip_suffix('p') {
        // nvme0n1p3 and mmcblk0p3: the disk's own name ends in a digit.
        Some(disk) if disk.ends_with(|c: char| c.is_ascii_digit()) => disk.to_string(),
        _ => digits.to_string(),
    }
}

/// The partition table of the disk `name`, of `bytes` in sectors of
/// `sector_size` bytes, as rdisk(8) reads it: `None` if it has none.
///
/// # Errors
///
/// If the disk cannot be read, or its GPT is damaged in both copies.
pub fn read_table(name: &str, bytes: u64, sector_size: u32) -> Result<Option<disk::Table>, String> {
    let file = std::fs::File::open(device(name))
        .map_err(|err| format!("{}: {err}", device(name).display()))?;
    let found = disk::read(&disk::Device::whole(file, bytes, sector_size))
        .map_err(|err| format!("{name}: {err}"))?;
    Ok(found.table)
}

/// The first sector of the partition `name` on its disk, in 512-byte
/// sectors, as the kernel has it now.
fn start(name: &str) -> Option<u64> {
    attribute(name, "start")?.parse().ok()
}

/// Waits until the partition `name` starts at `first`, in 512-byte
/// sectors, as after a table that moved it was written: a partition
/// taken out and made again under its number keeps its name.
///
/// # Errors
///
/// If it does not, after [`APPEAR`].
pub fn wait_for_start(name: &str, first: u64) -> Result<(), String> {
    let started = Instant::now();
    while start(name) != Some(first) || !device(name).exists() {
        if started.elapsed() > APPEAR {
            return Err(format!(
                "{} did not appear at sector {first} after its table was written",
                device(name).display()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

/// The device file of the disk or partition `name`.
#[must_use]
pub fn device(name: &str) -> PathBuf {
    Path::new("/devices").join(name)
}

/// The medium the live system started from, which holds the signed
/// release: the install stick's DUKOVO_LIVE partition, or the install DVD
/// (ADR 0433).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Medium {
    /// Its name in `/devices`: the stick's partition, or the DVD's drive.
    pub name: String,
    /// Where it is mounted, read-only.
    pub mounted: PathBuf,
    /// The disk it is on, which is never installed onto: the stick, or
    /// the drive.
    pub disk: String,
    /// Whether it is the install DVD.
    pub disc: bool,
}

/// The medium the live system started from: the stick's DUKOVO_LIVE
/// partition, or a drive with the install DVD in it.
///
/// # Errors
///
/// If there is neither, as when the system was not started from an
/// install stick or DVD.
pub fn live_medium() -> Result<Medium, String> {
    for name in names()? {
        let stick =
            attribute(&name, "parttype").is_some_and(|kind| kind.eq_ignore_ascii_case(DUKOVO_LIVE));
        let disc = !stick && is_live_disc(&name);
        if stick || disc {
            let disk = if disc { name.clone() } else { disk_of(&name) };
            return Ok(Medium { mounted: mounted_at(&name), disk, disc, name });
        }
    }
    Err("no disk has a DUKOVO_LIVE partition, and no drive holds the install DVD: start \
         the computer from the install stick or DVD, or give the release's folder with \
         --release"
        .into())
}

/// Whether `name` is a drive holding the install DVD: an optical drive
/// whose disc has an ISO 9660 volume labelled DUKOVO_LIVE, as `ob image
/// --iso` makes it (ADR 0433).
fn is_live_disc(name: &str) -> bool {
    if !is_disc(name) {
        return false;
    }
    let Ok(file) = std::fs::File::open(device(name)) else { return false };
    let mut pvd = [0u8; ISO_PVD_LEN];
    file.read_exact_at(&mut pvd, ISO_PVD).is_ok() && is_live_volume(&pvd)
}

/// Whether `pvd`, a disc's sector 16, is the primary volume descriptor of
/// an ISO 9660 volume labelled DUKOVO_LIVE.
fn is_live_volume(pvd: &[u8]) -> bool {
    pvd.len() >= ISO_VOLUME_ID.end
        && pvd.starts_with(ISO_PVD_START)
        && pvd[ISO_VOLUME_ID].trim_ascii_end() == LIVE_LABEL.as_bytes()
}

/// Where the live medium `name` is mounted: at its name, where Spark
/// mounts it as the live system starts, or else at its label, where
/// volumed mounts it when it is put in again.
fn mounted_at(name: &str) -> PathBuf {
    let volumes = Path::new("/volumes");
    let label = volumes.join(LIVE_LABEL);
    if !volumes.join(name).exists() && label.join("release").is_dir() {
        label
    } else {
        volumes.join(name)
    }
}

/// Waits until the device file of `name` is there, as after a partition
/// table is written, for [`APPEAR`] at most.
///
/// # Errors
///
/// If it does not appear.
pub fn wait_for(name: &str) -> Result<(), String> {
    let started = Instant::now();
    while !device(name).exists() {
        if started.elapsed() > APPEAR {
            return Err(format!(
                "{} did not appear after its table was written",
                device(name).display()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_partitions_as_the_kernel_does() {
        assert_eq!(partition("vda", 3), "vda3");
        assert_eq!(partition("sdb", 1), "sdb1");
        assert_eq!(partition("nvme0n1", 2), "nvme0n1p2");
        assert_eq!(partition("mmcblk0", 4), "mmcblk0p4");
        for (part, disk) in
            [("vda3", "vda"), ("sdb12", "sdb"), ("nvme0n1p2", "nvme0n1"), ("mmcblk0p4", "mmcblk0")]
        {
            assert_eq!(disk_of(part), disk);
        }
    }

    #[test]
    fn knows_the_install_dvd_by_its_volume_label() {
        let mut pvd = vec![b' '; ISO_PVD_LEN];
        pvd[..6].copy_from_slice(ISO_PVD_START);
        pvd[40..40 + LIVE_LABEL.len()].copy_from_slice(LIVE_LABEL.as_bytes());
        assert!(is_live_volume(&pvd));
        // Another label, a label that only starts so, another descriptor
        // type, and no ISO 9660 at all.
        let mut other = pvd.clone();
        other[40..51].copy_from_slice(b"DUKOVO_0_14");
        assert!(!is_live_volume(&other));
        let mut longer = pvd.clone();
        longer[51] = b'X';
        assert!(!is_live_volume(&longer));
        let mut supplementary = pvd.clone();
        supplementary[0] = 2;
        assert!(!is_live_volume(&supplementary));
        assert!(!is_live_volume(&[0; ISO_PVD_LEN]));
        assert!(!is_live_volume(&pvd[..40]));
    }
}
