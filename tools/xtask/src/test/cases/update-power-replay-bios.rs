// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! As *update-power-replay*, under a BIOS: GRUB's *core.img* in the BIOS
//! boot partition starts each slot, so the cuts also test what the BIOS
//! boot path finds when the slot state and the slots on the disk come
//! from different moments (ADR 0536).

use super::super::*;
use super::update_power_replay::record;
use super::update_stage::UPDATE_DISKS;

pub const CASE: Case = Case {
    name: "update-power-replay-bios",
    about: "as update-power-replay, under a BIOS: each power cut of the update starts slot A or slot B whole, never a mix (ADR 0536)",
    firmware: Firmware::Bios,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: record,
    disks: UPDATE_DISKS,
    second: None,
};
