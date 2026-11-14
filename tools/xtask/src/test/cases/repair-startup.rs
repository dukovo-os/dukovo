// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Repair startup makes an EFI system partition whose FAT cannot be read
//! again (ADR 0503). The live system installs Dukovo onto a blank disk as
//! install-uefi does, then overwrites the first 4 MiB of the EFI system
//! partition, `vda2`, with zeros, which takes its boot sector, both FATs
//! and its root folder: the partition cannot be mounted any more.
//! *dukovo-install --repair-startup*, which the install stick's Repair
//! startup entry runs, finds the Dukovo root on `vda`, says that the
//! partition cannot be read and what making it again erases, and leaves
//! it alone unless the disk's name, `vda`, is typed: given its partition's
//! name, `vda2`, it leaves it as it was, which is checked after the
//! restart it ends with. Given `vda`, it makes the partition again as
//! FAT32, labelled as an install labels it, puts GRUB, the menu, the slot
//! state and Rescue's files from the stick back, and says so. The stick
//! restarts the live system each time; the partition then mounts and
//! holds the menu, Rescue's entry and its files. On the host, the disk
//! must be as an install leaves it: the partition sound for dosfstools'
//! *fsck.fat* with UEFI's GRUB, a menu naming the root by its UUID, and a
//! root Linux's *btrfs check* finds sound. The second boot is from that
//! disk alone, without the stick, as install-uefi's: the system starts,
//! alice signs in and finds her settings, which nothing in the root
//! changed.

use super::super::*;
use super::install_uefi::{
    ANSWERS, INSTALL_DISKS, installed, live_shell, nothing, wait_for_screen,
};

pub const CASE: Case = Case {
    name: "repair-startup",
    about: "Repair startup (ADR 0503) makes an EFI system partition whose FAT cannot be read again, but only once the disk's name is typed: the partition is left alone when the partition's name is typed, and with the disk's name is made again as FAT32 with GRUB, the menu, the slot state and Rescue's files from the stick, which the host's fsck.fat and btrfs check find sound and from which the installed system starts again",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // The live menu entry has its own flags, `live` and
    // `console-sign-in=off`, before these.
    cmdline: "",
    cpus: 2,
    memory: "1G",
    // The stick restarts the live system after each repair, as it does
    // when its person presses Enter.
    reboot: true,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &[],
    script: repair,
    disks: INSTALL_DISKS,
    second: Some(SecondBoot { cmdline: "", before: nothing, script: installed }),
};

/// How long the install may take, in seconds: it copies the whole system.
const INSTALL_SECONDS: u64 = 240;

/// How long a repair may take: it checks the stick's signed release to put
/// Rescue's files back, and writes them.
const REPAIR_SECONDS: u64 = 240;

fn repair(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    live_shell(vm)?;
    vm.type_text(&format!(
        "dukovo-install --answers {ANSWERS} > /tmp/out 2>&1; echo \"install: $((100 + $?))\"\n"
    ))?;
    wait_for_screen(vm, "install: 1", INSTALL_SECONDS)?;
    if !vm.screen()?.iter().any(|line| line.trim() == "install: 100") {
        return Err(format!("dukovo-install failed: {}", vm.screen()?.join(" | ")));
    }

    // The EFI system partition's boot sector, FATs and root folder go,
    // and the partition cannot be mounted. The status is shown as 100 more
    // than it is, so that the command line the shell shows does not hold
    // what the test waits for.
    vm.type_text(
        "clear; dd if=/devices/zero of=/devices/vda2 bs=1M count=4 > /tmp/out 2>&1; \
         echo \"wiped: $((100 + $?))\"\n",
    )?;
    wait_for_screen(vm, "wiped: 10", 60)?;
    vm.expect_screen(&["wiped: 100"])?;
    let mounted = mounts(vm, "before")?;
    if mounted {
        return Err("vda2 mounts after its first 4 MiB were overwritten with zeros".into());
    }

    // The disk's name is not typed: its partition's is. The partition is
    // left as it is, and the repair says so.
    repair_run(vm, "vda2")?;
    vm.expect_screen(&[
        "Repair startup could not repair vda",
        "What it ran is in /tmp/install/repair-startup.log.",
    ])?;
    restart(vm)?;
    if mounts(vm, "left")? {
        return Err("vda2 mounts after a repair that was not told to make it again".into());
    }

    // The disk's name: the partition is made again and filled.
    repair_run(vm, "vda")?;
    vm.expect_screen(&[
        "vda2 could not be read: it is made again, as FAT32.",
        &format!("Rescue of Dukovo {VERSION} is on vda2 again."),
        "GRUB is back on the disk for UEFI",
        "Startup is repaired.",
    ])?;
    restart(vm)?;

    // The partition mounts, and holds the menu and Rescue's entry and files.
    if !mounts(vm, "after")? {
        return Err("vda2 does not mount after Repair startup made it again".into());
    }
    vm.type_text(
        "clear; mount -t fat vda2 /tmp/look && ls /tmp/look/boot/grub /tmp/look/boot/rescue; \
         umount /tmp/look; echo \"listed: $((100 + $?))\"\n",
    )?;
    vm.expect_screen(&["grub.cfg", "rescue.cfg", "state1", "state2", "startup", "listed: 100"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Runs Repair startup in the live system's shell and answers its question
/// with `answer`, then waits for it to ask for the stick to be pulled.
fn repair_run(vm: &mut Vm, answer: &str) -> Result<(), String> {
    vm.type_text("clear; dukovo-install --repair-startup\n")?;
    wait_for_screen(vm, "press Enter alone: ", 120)?;
    vm.expect_screen(&[
        "The EFI system partition vda2 of vda cannot be read.",
        "erases everything on vda2",
        "type the disk's name, vda, and press Enter.",
    ])?;
    vm.type_text(&format!("{answer}\n"))?;
    wait_for_screen(vm, "Remove the stick, then press Enter to restart.", REPAIR_SECONDS)
}

/// Presses Enter, which restarts the machine, from the stick again: the
/// live system's shell is back.
fn restart(vm: &mut Vm) -> Result<(), String> {
    vm.press(&["ret"])?;
    vm.expect("spark: asked to restart the machine")?;
    live_shell(vm)
}

/// Whether `vda2` mounts as FAT now, tried under `label`, which tells this
/// try's line from another's on the screen.
fn mounts(vm: &mut Vm, label: &str) -> Result<bool, String> {
    vm.type_text(&format!(
        "clear; mkdir -p /tmp/look; mount -t fat vda2 /tmp/look > /tmp/out 2>&1; \
         echo \"{label} mount: $((100 + $?))\"\n"
    ))?;
    wait_for_screen(vm, &format!("{label} mount: 1"), 60)?;
    let mounted = vm.screen()?.iter().any(|line| line.trim() == format!("{label} mount: 100"));
    if mounted {
        // Left mounted, the partition would be in use when the next step
        // overwrites or unmounts it.
        vm.type_text("clear; umount /tmp/look; echo \"unmounted: $((100 + $?))\"\n")?;
        vm.expect_screen(&["unmounted: 100"])?;
    }
    Ok(mounted)
}
