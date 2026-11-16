// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! As *update-first-start-kills*, under a BIOS: the resets of the update's
//! first start fall between GRUB's *core.img* in the BIOS boot partition
//! and the slots it starts (ADR 0536).

use super::super::*;
use super::update_first_start_kills::kill_the_first_start;
use super::update_stage::UPDATE_DISKS;

pub const CASE: Case = Case {
    name: "update-first-start-kills-bios",
    about: "as update-first-start-kills, under a BIOS: the machine is reset at random times of an update's first start, again and again, and then starts slot A or slot B whole (ADR 0536)",
    firmware: Firmware::Bios,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: kill_the_first_start,
    disks: UPDATE_DISKS,
    second: None,
};
