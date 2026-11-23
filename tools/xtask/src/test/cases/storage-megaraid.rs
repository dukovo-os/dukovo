// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The storage test on four volumes of QEMU's model of a MegaRAID SAS
//! 2108 controller (`megasas-gen2`), the last partitioned with a GPT: the
//! kernel brings the controller's firmware up through its MFI interface,
//! asks it for its volumes, one at each of targets 0, 1, 2 and 5, names
//! them `sda` to `sdd` in that order, and the files written in the first
//! boot are checked in the second (ADR 0664). The four share the
//! controller's commands, which the storage test's writes to all of them
//! at once keep busy, and reads and writes of more than a page go through
//! the frames' lists of runs.

use super::super::*;

pub const CASE: Case = Case {
    name: "storage-megaraid",
    about: "The storage test on four volumes of QEMU's MegaRAID SAS 2108 model, one of them partitioned with a GPT",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep -- /system/tests/storage write",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[MEGARAID],
    script: storage_write,
    disks: DISKS,
    second: Some(SecondBoot {
        cmdline: "lockdep -- /system/tests/storage check",
        before: storage_host,
        script: storage_check,
    }),
};

/// The storage test's disks, volumes of the controller ([`Bus::Scsi`] with
/// [`MEGARAID`]) at targets 0, 1, 2 and 5.
const DISKS: &[Disk] = &[
    fat_disk(1440, 12, "RADIX12", Table::None, Bus::Scsi(0)),
    fat_disk(32 * 1024, 16, "RADIX16", Table::None, Bus::Scsi(1)),
    fat_disk(64 * 1024, 32, "RADIX32", Table::None, Bus::Scsi(2)),
    fat_disk(48 * 1024, 32, "RADIXPART", Table::Gpt, Bus::Scsi(5)),
];
