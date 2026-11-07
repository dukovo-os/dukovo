// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Windows on a disk, as installing Dukovo next to it needs to know it
//! (ADR 0314): its volume, C:, with the space its files use and the
//! smallest it shrinks to; whether it can be shrunk now; and the EFI
//! system partition that holds Windows Boot Manager, which the boot menu
//! starts Windows from. Everything is read, nothing written: the volume is
//! checked by the kernel's own NTFS crate, as resize-ntfs(8) checks it.

use std::fmt;

use disk::{Device, Scheme, types};

use crate::disks;
use crate::machine::DiskInfo;

/// Where Windows Boot Manager is on an EFI system partition.
pub const BOOT_MANAGER: &str = "EFI/Microsoft/Boot/bootmgfw.efi";

/// What an NTFS volume's boot sector says at byte 3, its maker's name.
const NTFS_OEM: &[u8; 8] = b"NTFS    ";

/// Why Windows' volume cannot be shrunk now. Each is put right in
/// Windows, and the installer then started again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// BitLocker encrypts it: what is on the disk is no NTFS a shrink
    /// can read.
    BitLocker,
    /// Windows hibernated with it, or shut down with Fast Startup, which
    /// is the same: Windows still holds it as it was, and would lose what
    /// a shrink changed.
    FastStartup,
    /// It is marked to be checked: Windows did not finish with it, and
    /// chkdsk must look at it first.
    Dirty,
    /// The check found it damaged: the first problem, in words.
    Damaged(String),
    /// Radix would not write it: why, in words.
    Kept(String),
    /// It could not be read: why, in words.
    Unreadable(String),
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::BitLocker => f.write_str(disk::ntfs::ENCRYPTED),
            Refused::FastStartup => f.write_str(
                "Windows hibernated, or shut down with Fast Startup: turn Fast Startup off in \
                 Windows' power options, then shut Windows down",
            ),
            Refused::Dirty => f.write_str(
                "Windows must check its volume first: run chkdsk C: /f in Windows, then shut \
                 Windows down",
            ),
            Refused::Damaged(problem) => write!(
                f,
                "its volume is damaged ({problem}): run chkdsk C: /f in Windows, then shut \
                 Windows down"
            ),
            Refused::Kept(why) | Refused::Unreadable(why) => f.write_str(why),
        }
    }
}

/// Windows on a disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Windows {
    /// The partition of its volume, C:: the largest of the disk's basic
    /// data partitions that holds NTFS, or that BitLocker encrypts.
    pub partition: u32,
    /// Its size, in bytes.
    pub bytes: u64,
    /// The bytes its files use: the clusters its bitmap marks in use.
    pub used: u64,
    /// The smallest it shrinks to, in bytes.
    pub smallest: u64,
    /// Why it cannot be shrunk now, if it cannot.
    pub refused: Option<Refused>,
    /// The EFI system partition that holds Windows Boot Manager.
    pub boot_manager: u32,
}

/// Windows on `disk`, if it is there: a GPT disk whose EFI system
/// partition holds Windows Boot Manager, and an NTFS volume, or one
/// BitLocker encrypts, in a basic data partition. A disk with neither is
/// no Windows' disk, whatever its partitions' types say.
#[must_use]
pub fn find(disk: &DiskInfo) -> Option<Windows> {
    let table = disk.table.as_ref().filter(|table| table.scheme == Scheme::Gpt)?;
    let kind = |alias: &str| types::parse(alias, Scheme::Gpt);
    let boot_manager = table
        .partitions
        .iter()
        .filter(|p| Some(p.kind) == kind("efi"))
        .find(|p| {
            open(disk, p.number, p.sectors())
                .is_ok_and(|device| disk::esp::has_file(device, BOOT_MANAGER).unwrap_or(false))
        })?
        .number;
    let mut data: Vec<&disk::Partition> =
        table.partitions.iter().filter(|p| Some(p.kind) == kind("windows-data")).collect();
    data.sort_by_key(|p| std::cmp::Reverse(p.sectors()));
    for partition in data {
        let Ok(device) = open(disk, partition.number, partition.sectors()) else { continue };
        let bytes = partition.sectors() * u64::from(disk.sector_size);
        let volume = |used, smallest, refused| Windows {
            partition: partition.number,
            bytes,
            used,
            smallest,
            refused,
            boot_manager,
        };
        if disk::ntfs::encrypted(&device) {
            return Some(volume(0, bytes, Some(Refused::BitLocker)));
        }
        let mut oem = [0u8; 8];
        if device.read_at(3, &mut oem).is_err() || oem != *NTFS_OEM {
            // FAT or exFAT data, which Windows does not start from.
            continue;
        }
        return Some(match disk::ntfs::info(&device) {
            Ok(info) => {
                let used = info.report.marked * u64::from(info.report.cluster_size);
                volume(used, info.smallest, refusal(&info))
            }
            Err(why) => volume(0, bytes, Some(Refused::Unreadable(why))),
        });
    }
    None
}

/// Why a volume of `info` cannot be shrunk now, if it cannot: the
/// first of the reasons, in the order a person puts them right in.
fn refusal(info: &disk::ntfs::Info) -> Option<Refused> {
    if info.hibernated {
        Some(Refused::FastStartup)
    } else if info.dirty {
        Some(Refused::Dirty)
    } else if let Some(problem) = info.report.problems.first() {
        Some(Refused::Damaged(problem.clone()))
    } else {
        info.kept.map(|why| Refused::Kept(why.to_string()))
    }
}

/// Partition `number` of `disk`, `sectors` long, to read.
fn open(disk: &DiskInfo, number: u32, sectors: u64) -> Result<Device, String> {
    let path = disks::device(&disks::partition(&disk.name, number));
    let file = std::fs::File::open(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(Device::whole(file, sectors * u64::from(disk.sector_size), disk.sector_size))
}
