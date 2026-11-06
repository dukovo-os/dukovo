// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! As *live-uefi*, under a BIOS: GRUB's *core.img* in the stick's BIOS
//! boot partition starts the live system (ADR 0308), which takes no disk's
//! root, mounts only the stick's DUKOVO_LIVE, read-only, and checks the
//! signed release on it against the root it trusts.

use super::super::*;

pub const CASE: Case = Case {
    name: "live-bios",
    about: "The install stick boots under BIOS from USB into the live system, which takes no disk's root and checks the stick's signed release whole",
    firmware: Firmware::Bios,
    media: Media::Usb,
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &[],
    script: super::live_uefi::live,
    disks: ROOT_DISKS,
    second: None,
};
