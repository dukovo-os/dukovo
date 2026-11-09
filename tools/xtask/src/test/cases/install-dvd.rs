// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! As *install-uefi*, from the install DVD under a BIOS: the ISO boots
//! from a DVD in an IDE drive, the secondary channel's master, as an
//! older PC has it (ADR 0432), into the live system, which installs
//! Dukovo onto a blank disk as the test's answers file says, from the
//! signed release on the disc, checking every file as it copies it (ADR
//! 0433). The second boot is from that disk alone, without the disc,
//! from the GRUB *rgrub-setup* put on it: alice signs in and finds the
//! install's settings.

use super::super::*;

pub const CASE: Case = Case {
    name: "install-dvd",
    about: "The install DVD boots under BIOS from an IDE DVD drive into the live system, which installs Dukovo onto a blank disk from the signed release on the disc; the disk then boots alone, where alice signs in and finds the install's settings",
    firmware: Firmware::Bios,
    media: Media::IdeCdrom,
    // The live menu entry has its own flags, `live` and
    // `console-sign-in=off`, before these.
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &[],
    script: install,
    disks: super::install_uefi::INSTALL_DISKS,
    second: Some(SecondBoot {
        cmdline: "",
        before: super::install_uefi::nothing,
        script: super::install_uefi::installed,
    }),
};

/// The first boot, from the DVD: the live system installs Dukovo onto
/// `vda` as the answers file says, then powers off.
fn install(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    super::install_uefi::live_shell(vm)?;
    super::install_uefi::install_from(vm, super::install_uefi::ANSWERS, None, "disc")
}
