// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! As *memory-test*, under a BIOS: GRUB starts memtest86+ from the
//! default slot through the Linux boot protocol, and it shows its version
//! and runs its first test on the menu's serial port (ADR 0488).

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "memory-test-bios",
    about: "the start menu's Troubleshooting starts memtest86+ from the default slot under a BIOS, on the menu's serial port, and it runs its first test (ADR 0488)",
    firmware: Firmware::Bios,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "512M",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: super::memory_test::memory_test,
    disks: &[],
    second: None,
};
