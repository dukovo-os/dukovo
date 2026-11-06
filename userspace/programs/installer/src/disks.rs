// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The disks, as the kernel lists them in `/kernel/block` (radix-sysfs(5)):
//! the install stick's DUKOVO_LIVE partition, which holds the release, the
//! disk it is on, which is never installed onto, and the names and sizes of
//! disks and their partitions.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The GPT type of an install stick's DUKOVO_LIVE partition, "Dukovo
/// live" (ADR 0308), as `/kernel/block/<name>/parttype` gives it.
pub const DUKOVO_LIVE: &str = "5894c662-af90-4b47-baaf-2e3396443b25";

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

/// The device file of the disk or partition `name`.
#[must_use]
pub fn device(name: &str) -> PathBuf {
    Path::new("/devices").join(name)
}

/// The install stick's DUKOVO_LIVE partition: its name and where Spark
/// mounted it, read-only.
///
/// # Errors
///
/// If no partition is one, as when the system was not started from an
/// install stick.
pub fn live_partition() -> Result<(String, PathBuf), String> {
    for name in names()? {
        if attribute(&name, "parttype").is_some_and(|kind| kind.eq_ignore_ascii_case(DUKOVO_LIVE)) {
            let mounted = Path::new("/volumes").join(&name);
            return Ok((name, mounted));
        }
    }
    Err("no disk has a DUKOVO_LIVE partition: start the computer from the install stick, \
         or give the release's folder with --release"
        .into())
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
}
