// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The ISO, as *ob image --iso* makes it, boots under UEFI from a DVD in
//! a drive on an AHCI port into the live system (ADR 0433), with a disk
//! holding a Dukovo root beside it, as *live-uefi* boots the stick. Spark
//! takes no disk's root, leaves the other disk unmounted, and mounts the
//! disc, labelled DUKOVO_LIVE, read-only. The signed release in the
//! disc's *release* directory checks against the root the live system
//! trusts, every file of it read from the disc, and nothing can be
//! written to it.

use super::super::*;

pub const CASE: Case = Case {
    name: "live-dvd",
    about: "The ISO boots under UEFI from a DVD drive into the live system: no disk's root is taken, only the disc labelled DUKOVO_LIVE is mounted, read-only, and the signed release on it checks whole against the trusted root",
    firmware: Firmware::Uefi,
    media: Media::Cdrom,
    // The live menu entry has its own flags, `live` and
    // `console-sign-in=off`, before these.
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
