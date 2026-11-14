// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The start menu's memory test (ADR 0488) under UEFI: memtest86+ from
//! the default slot's startup folder, as Troubleshooting's "Test the
//! memory" starts it. The installed system makes that entry the menu's
//! default on its EFI system partition and restarts; GRUB starts
//! memtest86+ as an EFI program, with the menu's serial port as its
//! console, and it shows its version and runs its first test there.

use super::super::*;
use super::update_stage::{restart, run, started};

/// The test.
pub const CASE: Case = Case {
    name: "memory-test",
    about: "the start menu's Troubleshooting starts memtest86+ from the default slot under UEFI, on the menu's serial port, and it runs its first test (ADR 0488)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "512M",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: memory_test,
    disks: &[],
    second: None,
};

/// The test disk's EFI system partition, its second.
const ESP: &str = "sda2";

/// Makes Troubleshooting's memory test the menu's default and restarts
/// into it.
pub fn memory_test(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    started(vm, 'a')?;
    run(
        vm,
        &format!(
            "mkdir -p /tmp/esp && mount -t fat {ESP} /tmp/esp && echo \
             'set default=\"troubleshooting>memory-test\"' >> /tmp/esp/boot/grub/grub.cfg && \
             umount /tmp/esp && sync && echo \"menu: $((6 * 7))\""
        ),
        &["menu: 42"],
    )?;
    restart(vm)?;
    vm.expect("Memtest86+ v8.10")?;
    // Its first test, after the few seconds it waits for a key that
    // would change its settings.
    vm.expect("[Address test, walking ones")?;
    Ok(())
}
