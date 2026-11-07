// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Under OVMF, whose variables persist across a restart, the kernel finds
//! the firmware's runtime services through the UEFI memory map GRUB
//! passes, and *efiboot* lists OVMF's own boot entries and the one it
//! started from. It makes an entry for the stick's GRUB, out of the boot
//! order, which *efibootmgr -v* shows by its partition's GPT GUID and
//! file; asks the firmware to start it next time alone, and sets the
//! menu's time. After *reboot* the firmware starts that entry, says so in
//! *BootCurrent*, and has forgotten *BootNext*, while the order never had
//! the entry. (OVMF sets the time from QEMU's at every start, as EDK II's
//! boot manager does, so the time set is not looked for again.) Deleted,
//! the entry is gone, and so is the time (ADR 0313).

use super::super::*;

pub const CASE: Case = Case {
    name: "efi-boot-entries",
    about: "The firmware's boot entries under OVMF: efiboot lists them, makes one for the stick's GRUB out of the order, sets the menu's time and has the firmware start the entry next time alone; after a restart the firmware started that entry and forgot BootNext, and the entry and the time delete",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: efi_boot_entries,
    disks: &[],
    second: None,
};

/// The number the test gives its entry: far from OVMF's own.
const ENTRY: &str = "0099";

/// The `efi-boot-entries` test.
fn efi_boot_entries(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("EFI: runtime services of UEFI 2.")?;
    console(vm)?;

    // OVMF's own entries, and the one this boot started from.
    vm.type_text("efiboot\n")?;
    vm.expect_screen(&["BootCurrent: 000", "BootOrder: 000", "Boot0000* UiApp"])?;

    // An entry for GRUB on the stick's EFI system partition, its second,
    // out of the order (-C).
    vm.type_text(&format!(
        "clear; d=$(ls /devices | grep -m1 '^sd[a-z]$'); efiboot -C -b {ENTRY} -d $d -p 2 \
         -L 'Dukovo test' -q; echo \"made: $((10 + $?))\"\n"
    ))?;
    vm.expect_screen(&["made: 10"])?;
    vm.type_text(&format!(
        "clear; efibootmgr -v | grep -F 'Boot{ENTRY}* Dukovo test' | grep -qF \
         '/File(\\EFI\\BOOT\\BOOTX64.EFI)' && efiboot -v | grep -q 'HD(2,GPT,[0-9a-f-]*,0x' \
         && echo \"listed: $((10 + 1))\"\n"
    ))?;
    vm.expect_screen(&["listed: 11"])?;
    vm.type_text(&format!(
        "clear; efiboot | grep BootOrder | grep -q {ENTRY} || echo \"out of order: $((10 + 2))\"\n"
    ))?;
    vm.expect_screen(&["out of order: 12"])?;

    // Started next time alone, with the menu's time set; the listing
    // after a change says both.
    vm.type_text(&format!("clear; efiboot -n {ENTRY} -t 3\n"))?;
    vm.expect_screen(&[&format!("BootNext: {ENTRY}"), "Timeout: 3 seconds"])?;
    vm.type_text("reboot\n")?;
    vm.expect("spark: asked to restart the machine")?;

    // The firmware started the new entry: the same GRUB, by its path.
    vm.expect("EFI: runtime services of UEFI 2.")?;
    console(vm)?;
    vm.type_text("efiboot\n")?;
    vm.expect_screen(&[&format!("BootCurrent: {ENTRY}")])?;
    vm.type_text("clear; efiboot | grep -q BootNext || echo \"next forgotten: $((20 + 1))\"\n")?;
    vm.expect_screen(&["next forgotten: 21"])?;
    vm.type_text(&format!(
        "clear; efiboot | grep BootOrder | grep -q {ENTRY} || echo \"still out: $((20 + 2))\"\n"
    ))?;
    vm.expect_screen(&["still out: 22"])?;

    // Deleted, the entry is gone; the time goes too.
    vm.type_text(&format!(
        "clear; efiboot -b {ENTRY} -B -q && efiboot -T -q; efiboot | grep -q 'Boot{ENTRY}\\|Timeout' \
         || echo \"deleted: $((20 + 3))\"\n"
    ))?;
    vm.expect_screen(&["deleted: 23"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Waits for the console's shell, root's, which holds the platform
/// capability (consoled's file asks for admin.hardware).
fn console(vm: &mut Vm) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])
}
