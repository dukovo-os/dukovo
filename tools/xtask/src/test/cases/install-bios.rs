// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! As *install-uefi*, under a BIOS: the live system installs Dukovo onto
//! a blank disk from the test's answers file (ADR 0309), and the second
//! boot, from that disk alone, starts from the GRUB *rgrub-setup* put in
//! its first sector and BIOS boot partition. alice signs in and finds
//! the install's settings.

use super::super::*;

pub const CASE: Case = Case {
    name: "install-bios",
    about: "As install-uefi, under a BIOS: the live system installs Dukovo onto a blank disk from an answers file, the host finds the disk sound, and the BIOS then boots it alone with the GRUB the installer put on it",
    firmware: Firmware::Bios,
    media: Media::Usb,
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &[],
    script: super::install_uefi::install,
    disks: super::install_uefi::INSTALL_DISKS,
    second: Some(SecondBoot {
        cmdline: "",
        before: super::install_uefi::nothing,
        script: super::install_uefi::installed,
    }),
};
