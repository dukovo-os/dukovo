// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The free-space layout under a BIOS (ADR 0311). The live system's shell
//! first gives the blank virtio disk a partition of data, an exFAT
//! holding a file, as a disk that is used already has. Then
//! *dukovo-install --answers* installs Dukovo in the free space after
//! it, as the test's answers file says: the guided layout's partitions,
//! the data partition kept as it was. The host finds the disk sound. The second
//! boot is from the install stick, still there, by its menu's *Boot from
//! first disk*, which starts the GRUB the installer put on the disk:
//! alice signs in, finds the install's settings and swap, and the file
//! on the data partition, which Spark mounted.

use super::super::*;
use super::install_uefi::{holds_install, power_off, signed_in, wait_for_screen};

pub const CASE: Case = Case {
    name: "install-free",
    about: "The live system installs Dukovo under a BIOS into the free space of a disk that holds a data partition, from an answers file; the data partition is kept, the host finds the disk sound, and the stick's Boot from first disk then starts it, where alice signs in and finds the file kept",
    firmware: Firmware::Bios,
    media: Media::Usb,
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &[],
    script: install,
    disks: FIRST_DISK,
    second: Some(SecondBoot { cmdline: "", before: super::install_uefi::nothing, script: installed }),
};

/// The blank disk, `vda`, 4 GiB as install-uefi's, which the second
/// boot starts by the stick's menu.
const FIRST_DISK: &[Disk] = &[Disk {
    kib: 4 * 1024 * 1024,
    fat: INSTALL_FIRST_DISK,
    cluster: 0,
    label: "",
    table: Table::None,
    bus: Bus::Virtio,
    sector: 512,
    device: "",
    errors: None,
    read_only: false,
    check: free_disk,
}];

/// The test's answers file: the free-space layout on `vda`, with 1 GiB
/// of swap, and otherwise install-uefi's answers.
const ANSWERS: &str = "/system/tests/install/free.json";

/// The data partition the shell makes before the install: 256 MiB, at
/// the disk's start.
const DATA: &str =
    r#"{"table": "gpt", "partitions": [{"type": "linux", "size": "256MiB", "name": "Kept"}]}"#;

/// The disk after the install, by partition number: the data partition,
/// then the guided layout's partitions in the free space, with the EFI
/// system partition second, where a BIOS's GRUB finds its files.
const FREE: &[&str] = &["linux", "efi", "bios-boot", "swap", "dukovo-root"];

/// How long the install may take, in seconds: it copies the whole system.
const INSTALL_SECONDS: u64 = 240;

/// The first boot, from the stick: a data partition on `vda`, then the
/// install into the free space beside it.
fn install(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: the live system: /system is the boot archive, in memory, and no disk is the root")?;
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    make_data(vm)?;

    vm.type_text(&format!(
        "clear; dukovo-install --answers {ANSWERS} > /tmp/out 2>&1; \
         echo \"install: $((100 + $?))\"\n"
    ))?;
    wait_for_screen(vm, "install: 1", INSTALL_SECONDS)?;
    if !vm.screen()?.iter().any(|line| line.trim() == "install: 100") {
        vm.type_text("clear; tail -n 8 /tmp/install.log\n")?;
        let _ = vm.expect_screen(&["problem: "]);
        return Err(format!("dukovo-install failed: {}", vm.screen()?.join(" | ")));
    }
    vm.type_text("clear; cat /tmp/out\n")?;
    vm.expect_screen(&["is installed on vda. Remove the install stick, then restart."])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Makes the data partition, `vda1`, an exFAT holding `kept.txt`, with
/// rdisk and mkfs as a person would. install-manual's first boot makes
/// its partitions the same way.
pub fn make_data(vm: &mut Vm) -> Result<(), String> {
    vm.type_text(&format!("echo '{DATA}' > /tmp/data.json\n"))?;
    vm.type_text("rdisk --script /tmp/data.json vda | grep kernel\n")?;
    vm.expect_screen(&["The kernel has read it"])?;
    vm.type_text(
        "clear; mkfs-exfat -L KEPT /devices/vda1 && mkdir /tmp/k && mount -t exfat vda1 /tmp/k && \
         echo still here > /tmp/k/kept.txt && umount /tmp/k; echo \"made: $((40 + $?))\"\n",
    )?;
    vm.expect_screen(&["made: 40"])
}

/// The second boot, from the stick, by Boot from first disk: the
/// installed system starts, alice signs in, and the data partition holds
/// its file.
fn installed(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("Booting `Boot from first disk'")?;
    signed_in(vm)?;
    kept(vm)?;
    power_off(vm)
}

/// The data partition, `vda1`, is mounted at /volumes/vda1 with its file,
/// as it was before the install. install-manual's second boot checks the
/// same.
pub fn kept(vm: &mut Vm) -> Result<(), String> {
    answer_prompt(vm, "$", "echo \"kept\"\": $(cat /volumes/vda1/kept.txt)\"")?;
    vm.expect_screen(&["kept: still here"])
}

/// The host's side, after each boot: the data partition, then the guided
/// layout's partitions, sound.
fn free_disk(disk: &DiskImage, log: &str) -> Result<(), String> {
    holds_install(disk, log, FREE)
}
