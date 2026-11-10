// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A disk that grows under its partition and Btrfs, as a cloud image's
//! does when it is copied to a larger disk (ADR 0327). The first boot
//! gives a blank virtio disk of 160 MiB a GPT with one partition of 128
//! MiB, makes a Btrfs on it and writes a file. Between the boots the host
//! makes the disk 384 MiB, which leaves the GPT's backup in the middle.
//! In the second boot Spark mounts the Btrfs, then *rdisk --grow* grows the
//! partition to the disk's end while it is mounted: the backup moves to
//! the end, and the kernel grows the partition in place. *device resize
//! max* grows the Btrfs over it, and a file larger than the partition was
//! at first fits. Grown once, it grows no more. *fsck-btrfs* finds the
//! Btrfs sound, and the host's *sgdisk* the table.

use std::process::Command;

use super::super::*;

pub const CASE: Case = Case {
    name: "disk-grow",
    about: "A disk grown between boots under a mounted Btrfs partition: rdisk --grow grows the partition to the disk's end in use, the kernel grows it in place, device resize max grows the Btrfs, a file larger than the partition was fits, fsck-btrfs and the host's sgdisk find all sound",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: made,
    disks: DISKS,
    second: Some(SecondBoot { cmdline: "console-sign-in=off", before: grown, script: grow }),
};

/// The disk as the first boot finds it: 160 MiB of bytes that hold no
/// table or filesystem.
const DISKS: &[Disk] = &[Disk {
    kib: 160 * 1024,
    fat: 0,
    cluster: 0,
    label: "",
    table: Table::None,
    bus: Bus::Virtio,
    sector: 512,
    device: "",
    errors: None,
    read_only: false,
    check: grown_table,
}];

/// The disk's size in the second boot.
const GROWN: u64 = 384 << 20;

/// A GPT with one partition of 128 MiB, named Root.
const LAYOUT: &str =
    r#"{"table": "gpt", "partitions": [{"type": "linux", "size": "128MiB", "name": "Root"}]}"#;

/// The first boot: the partition, its Btrfs and a file on it.
fn made(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text(&format!("echo '{LAYOUT}' > /tmp/root.json\n"))?;
    vm.type_text("rdisk --script /tmp/root.json vda | grep kernel\n")?;
    vm.expect_screen(&["The kernel has read it"])?;
    vm.expect("Disk: vda1: 128 MiB from sector 2048, GPT \"Root\"")?;
    vm.type_text(
        "mkfs-btrfs -q vda1 && mkdir /tmp/r && mount vda1 /tmp/r && echo kept > /tmp/r/kept \
         && umount /tmp/r && echo \"made: $((40 + 2))\"\n",
    )?;
    vm.expect_screen(&["made: 42"])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Between the boots: the disk grows to [`GROWN`], as a cloud's does.
fn grown(_case: &Case, disks: &[DiskImage]) -> Result<(), String> {
    let Some(disk) = disks.first() else { return Err("the case has no disk".into()) };
    std::fs::File::options()
        .write(true)
        .open(&disk.path)
        .and_then(|file| file.set_len(GROWN))
        .map_err(|err| format!("growing {}: {err}", disk.path.display()))
}

/// The second boot: the partition and the Btrfs grown, in use.
fn grow(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("filesystem of vda1 at /volumes/vda1")?;
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text("cat /volumes/vda1/kept\n")?;
    vm.expect_screen(&["kept"])?;
    // The partition grows to the disk's last usable sector, 786398.
    vm.type_text("clear; rdisk --grow vda\n")?;
    vm.expect_screen(&["/devices/vda1 grows from 128 MiB to 382.9 MiB.", "The kernel has read it"])?;
    vm.expect("Disk: vda1 grew to 382.9 MiB, in place")?;
    vm.expect("Disk: vda's partition table read again: a GPT with 1 partition, 0 new, 1 grown, 0 gone")?;
    vm.type_text("clear; device resize max /volumes/vda1\n")?;
    vm.expect_screen(&["Resized device id 1 of '/volumes/vda1' to 382.9 MiB"])?;
    vm.expect("Btrfs: disk 1 resized; it is 382.9 MiB now")?;
    // 200 MB: more than the partition held before it grew.
    vm.type_text("head -c 200000000 /devices/zero > /volumes/vda1/big && sync && echo \"wrote: $((40 + 3))\"\n")?;
    vm.expect_screen(&["wrote: 43"])?;
    vm.type_text("clear; rdisk --grow vda 1; device resize 1:max /volumes/vda1\n")?;
    vm.expect_screen(&[
        "/devices/vda1 already ends where it can: it is 382.9 MiB.",
        "Resized device id 1 of '/volumes/vda1' to 382.9 MiB",
    ])?;
    vm.type_text("clear; umount /volumes/vda1 && fsck-btrfs vda1 | tail -4 | head -1\n")?;
    vm.expect_screen(&["no error found"])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Checks on the host, after each boot, that sgdisk finds the table
/// sound, and that partition 1 is 128 MiB after the first and ends at
/// the grown disk's last usable sector after the second.
fn grown_table(disk: &DiskImage, _log: &str) -> Result<(), String> {
    let said = host_tool(Command::new(fat_tool("sgdisk")).arg("-v").arg(&disk.path))?;
    let said = String::from_utf8_lossy(&said);
    if !said.contains("No problems found") {
        return Err(format!("sgdisk -v found problems in the table:\n{said}"));
    }
    let bytes = std::fs::metadata(&disk.path)
        .map_err(|err| format!("{}: {err}", disk.path.display()))?
        .len();
    let last = if bytes == GROWN { "Last sector: 786398 " } else { "Last sector: 264191 " };
    let said = host_tool(Command::new(fat_tool("sgdisk")).arg("-i").arg("1").arg(&disk.path))?;
    let said = String::from_utf8_lossy(&said);
    if !said.contains(last) {
        return Err(format!("sgdisk does not give partition 1's \"{last}\":\n{said}"));
    }
    Ok(())
}
