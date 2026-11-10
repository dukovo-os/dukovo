// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The storage test on four disks at targets of QEMU's virtio SCSI
//! adapter, as cloud machines attach theirs, the last partitioned with a
//! GPT: the kernel scans every target, finds a disk at each of 0, 1, 2
//! and 5, names them `sda` to `sdd` in that order, and the files written
//! in the first boot are checked in the second (ADR 0327). The four share
//! the adapter's one queue, which the storage test's writes to all of them
//! at once keep full.

use super::super::*;

pub const CASE: Case = Case {
    name: "storage-scsi",
    about: "The storage test on four disks at targets of a virtio SCSI adapter, one of them partitioned with a GPT",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep -- /system/tests/storage write",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: storage_write,
    disks: DISKS,
    second: Some(SecondBoot {
        cmdline: "lockdep -- /system/tests/storage check",
        before: storage_host,
        script: storage_check,
    }),
};

/// The storage test's disks on a virtio SCSI adapter ([`Bus::Scsi`]), at
/// targets 0, 1, 2 and 5: the scan passes the empty ones between.
const DISKS: &[Disk] = &[
    fat_disk(1440, 12, "RADIX12", Table::None, Bus::Scsi(0)),
    fat_disk(32 * 1024, 16, "RADIX16", Table::None, Bus::Scsi(1)),
    fat_disk(64 * 1024, 32, "RADIX32", Table::None, Bus::Scsi(2)),
    fat_disk(48 * 1024, 32, "RADIXPART", Table::Gpt, Bus::Scsi(5)),
];
