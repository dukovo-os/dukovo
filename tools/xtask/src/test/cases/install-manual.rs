// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The manual layout under UEFI, on the installer's rdisk screen, by keys
//! (ADR 0311). The live system's shell first gives the blank virtio disk
//! three partitions as a used disk has: data to keep, an exFAT holding a
//! file, one to take out, and one to format. *dukovo-install* then shows
//! its screens, and Manual opens the rdisk screen, where D takes out the
//! second partition, F formats the third for exFAT data, and N makes the
//! EFI system partition in the second's place, a swap partition of a
//! size typed, and the Dukovo root in the rest. The review lists each
//! change, and the install makes them. The host finds the disk sound. The
//! second boot is from the install stick, still there, by its menu's
//! *Boot from first disk*, which starts the disk's UEFI GRUB: alice signs
//! in with the password typed in the screens, the swap is on, and the
//! first partition still holds its file.

use super::super::*;
use super::install_free::{kept, make_data};
use super::install_uefi::{holds_install, power_off, wait_for_screen};

pub const CASE: Case = Case {
    name: "install-manual",
    about: "The installer's rdisk screen, by keys under UEFI, keeps a data partition, takes one out, formats one and makes the EFI system, swap and root partitions; the install makes those changes, the host finds the disk sound, and the stick's Boot from first disk then starts it, where alice signs in and finds the kept file and the swap on",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &[],
    script: screens,
    disks: FIRST_DISK,
    second: Some(SecondBoot { cmdline: "", before: super::install_uefi::nothing, script: installed }),
};

/// The blank disk, `vda`, 4 GiB as install-uefi's, which the second
/// boot starts by the stick's menu. It is in the firmware's boot order
/// after the stick, as a machine's own disk is: OVMF makes ready only
/// the disks QEMU's boot order names.
const FIRST_DISK: &[Disk] = &[Disk {
    kib: 4 * 1024 * 1024,
    fat: INSTALL_FIRST_DISK,
    cluster: 0,
    label: "",
    table: Table::None,
    bus: Bus::Virtio,
    sector: 512,
    device: ",bootindex=1",
    errors: None,
    read_only: false,
    check: manual_disk,
}];

/// The two partitions the shell adds after the data partition: one that
/// the rdisk screen takes out, 512 MiB, where the EFI system partition
/// goes, and one it formats.
const MORE: &str = r#"{"table": "gpt", "keep": true, "partitions": [{"type": "linux", "size": "512MiB", "name": "Old"}, {"type": "linux", "size": "256MiB", "name": "Scratch"}]}"#;

/// The disk after the install: the data kept, the EFI system partition
/// in the old one's place, the data formatted, the swap and the root.
const MANUAL: &[&str] = &["linux", "efi", "linux", "swap", "dukovo-root"];

/// alice's password, typed into the screens.
const PASSWORD: &str = "queen of hearts";

/// How long the install may take, in seconds: it copies the whole system.
const INSTALL_SECONDS: u64 = 240;

/// The first boot, from the stick: the partitions made in the shell,
/// then the screens, the rdisk screen's changes and the install.
fn screens(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: the live system: /system is the boot archive, in memory, and no disk is the root")?;
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    make_data(vm)?;
    vm.type_text(&format!("echo '{MORE}' > /tmp/more.json\n"))?;
    vm.type_text("clear; rdisk --script /tmp/more.json vda | grep kernel\n")?;
    vm.expect_screen(&["The kernel has read it"])?;

    // Welcome and Network as they are; the Disk page's Manual.
    vm.type_text("clear; dukovo-install\n")?;
    vm.expect_screen(&["Step 1 of 6", "How do you want to install?"])?;
    vm.press(&["ret"])?;
    vm.expect_screen(&["Step 2 of 6"])?;
    vm.press(&["ret"])?;
    vm.expect_screen(&["Step 3 of 6", "vda", "Manual: choose the partitions yourself"])?;
    // The disk, then Manual: the second choice after the guided layout,
    // or the third if the disk's free space is room enough for the
    // free-space layout, which the size of the release decides.
    vm.press(&["spc", "tab", "tab", "spc"])?;
    // The test screen shows a chosen choice's dot as `?`.
    if !vm.screen()?.iter().any(|line| line.contains("(?) Manual")) {
        vm.press(&["tab", "spc"])?;
    }
    vm.expect_screen(&["(?) Manual"])?;
    vm.press(&["ret"])?;
    vm.expect_screen(&["Partitions (rdisk)", "Kept", "Old", "Scratch", "Pending changes: 0"])?;

    // D takes out the second; F formats the third for exFAT data.
    vm.press(&["down", "d", "down", "f"])?;
    vm.expect_screen(&["Format a partition", "Data: Btrfs"])?;
    vm.press(&["right", "ret"])?;
    vm.expect_screen(&["delete partition 2, Old", "format partition 3 as Data: exFAT"])?;

    // N offers the EFI system partition first, at 512 MiB, which goes in
    // the space the second left.
    vm.press(&["n"])?;
    vm.expect_screen(&["New partition", "EFI system: FAT32 at /system/boot", "512 MiB"])?;
    vm.press(&["ret"])?;
    vm.expect_screen(&["make EFI system on vda, 512 MiB"])?;
    // Then the root, turned into swap by the use, with a size typed.
    vm.press(&["n"])?;
    vm.expect_screen(&["Dukovo root: Btrfs at /"])?;
    vm.press(&["right", "right"])?;
    vm.press(&["backspace"; 12])?;
    vm.type_text("256 MiB")?;
    vm.press(&["ret"])?;
    vm.expect_screen(&["make Swap on vda, 256 MiB"])?;
    // Then the root, in the rest.
    vm.press(&["n", "ret"])?;
    vm.expect_screen(&["make Dukovo root on vda", "Pending changes: 5"])?;
    vm.press(&["ret"])?;

    // Software: Server as it starts, on to You: Up from the first field
    // wraps round to Help, then Next.
    vm.expect_screen(&["Step 4 of 6", "What is this machine for?"])?;
    vm.press(&["up", "up", "ret"])?;

    // You: the name makes the user name.
    vm.expect_screen(&["Step 5 of 6", "The administrator account."])?;
    vm.type_text("Alice Liddell")?;
    vm.press(&["tab", "tab"])?;
    vm.type_text(PASSWORD)?;
    vm.press(&["tab"])?;
    vm.type_text(PASSWORD)?;
    vm.press(&["ret"])?;

    // Review: each change, and UEFI's GRUB alone, as the disk has no
    // BIOS boot partition.
    vm.expect_screen(&[
        "Step 6 of 6",
        "Change    vda",
        "delete  2  Old",
        "make    2  EFI system",
        "format  3  Scratch",
        "make    4  Swap",
        "make    5  Dukovo root",
        "GRUB for UEFI (the disk has no BIOS boot partition)",
        "Type vda to confirm",
    ])?;
    vm.type_text("vda")?;
    vm.press(&["ret"])?;
    vm.expect_screen(&["Installing Dukovo"])?;
    if let Err(err) = wait_for_screen(vm, "Remove the install stick, then restart.", INSTALL_SECONDS) {
        vm.press(&["alt-f2"])?;
        vm.expect_screen(&["# "])?;
        vm.type_text("clear; tail -n 12 /tmp/install.log\n")?;
        let _ = vm.expect_screen(&["problem: "]);
        return Err(format!("{err}; the log: {}", vm.screen()?.join(" | ")));
    }

    vm.press(&["alt-f2"])?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty2)"), "# "])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The second boot, from the stick, by Boot from first disk: the disk's
/// UEFI GRUB starts the installed system, alice signs in with the
/// password typed in the screens, the swap partition made on the rdisk
/// screen is on, and the kept partition holds its file.
fn installed(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("Booting `Boot from first disk'")?;
    vm.expect("spark: /system is Dukovo's root on vda5")?;
    vm.expect("spark: turned on swap on vda4, a swap partition of vda")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION}")])?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", PASSWORD)?;
    vm.expect_anywhere("authd: alice signed in on pts/")?;
    kept(vm)?;
    // The formatted partition is an exFAT, mounted too.
    answer_prompt(vm, "$", "echo \"exfat\"\": $(mount -t exfat | grep -c /volumes/vda)\"")?;
    vm.expect_screen(&["exfat: 2"])?;
    power_off(vm)
}

/// The host's side, after each boot: the partitions the rdisk screen
/// left, sound.
fn manual_disk(disk: &DiskImage, log: &str) -> Result<(), String> {
    holds_install(disk, log, MANUAL)
}
