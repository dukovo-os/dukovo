// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Two DVD drives on QEMU's PIIX3 IDE controller (ADR 0432): one the
//! slave of the primary channel, whose master is a FAT disk, the other
//! alone on the secondary channel, each with a disc in it at boot. The
//! kernel names the drives cd0 and cd1, reads each disc whole and
//! read-only, and Spark mounts each at the drive's name in `/volumes`.
//! A file copied from the disc to the disk on its channel, with the two
//! sharing the channel one command at a time, reads back as on the disc;
//! the discs' big files are the same. While mounted, a disc is locked
//! in, and QEMU, refused, presses the drive's eject button: volumed
//! unmounts the disc and has the tray opened. A disc put in again is
//! mounted at its label, and one pulled out by force is unmounted all
//! the same; the disk on the channel answers after.

use std::time::Duration;

use super::super::*;

pub const CASE: Case = Case {
    name: "optical-ide",
    about: "Discs in two DVD drives on an IDE controller, one sharing its channel with a disk, are read whole and mounted at boot; a file is copied from a disc to the disk, a disc is locked in while mounted, ejected by the drive's eject button, mounted at its label when put in again, and unmounted when pulled out by force",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // No program: Spark and volumed mount, as on a normal boot.
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: optical_ide,
    disks: DISKS,
    second: None,
};

/// The disk as the primary channel's master (sda, QEMU's disk0), a disc
/// in the drive that is its slave (cd0, disk1), and one in the drive
/// that is the secondary channel's master (cd1, disk2).
const DISKS: &[Disk] = &[
    fat_disk(16 * 1024, 16, "RADIXIDE", Table::None, Bus::Ide),
    disc(ISO9660, "RADIXPRI"),
    disc(UDF, "RADIXSEC"),
];

/// A disc image of 4 MiB in an IDE DVD drive, which QEMU says is a
/// CD-ROM.
const fn disc(fat: u8, label: &'static str) -> Disk {
    Disk {
        kib: 4096,
        fat,
        cluster: 0,
        label,
        table: Table::None,
        bus: Bus::IdeCd,
        sector: 512,
        device: "",
        errors: None,
        read_only: false,
        check: btrfs_unchanged,
    }
}

/// What `/kernel/block` and lsblk say of `drive`, on the screen: its
/// size, what disc is in it, its count of media changes and its type.
fn drive_says(vm: &mut Vm, drive: &str, size: &str, disc: &str, changes: u32) -> Result<(), String> {
    let at = format!("/kernel/block/{drive}");
    vm.type_text(&format!(
        "clear; echo \"{drive} size $(cat {at}/size) disc $(cat {at}/disc) changes $(cat {at}/media_changes) $(lsblk -d -n -o TYPE {drive})\"\n"
    ))?;
    vm.expect_screen(&[&format!("{drive} size {size} disc {disc} changes {changes} rom")])
}

fn optical_ide(vm: &mut Vm, case: &Case) -> Result<(), String> {
    disks_found(vm, case)?;
    vm.expect_anywhere("spark: mounted the FAT filesystem of sda at /volumes/sda")?;
    for (disk, name) in case.disks[1..].iter().zip(["cd0", "cd1"]) {
        let (kind, said) = disk.kind();
        let label = disk.label;
        vm.expect_anywhere(&format!(
            "Files: mounted {kind} from {name} on /volumes/{name}, read-only, labelled {label}"
        ))?;
        vm.expect_anywhere(&format!(
            "spark: mounted the {said} filesystem of {name} at /volumes/{name}, read-only"
        ))?;
    }
    vm.expect_anywhere("volumed: watching for volumes; ")?;
    vm.expect_anywhere("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    for drive in ["cd0", "cd1"] {
        vm.type_text(&format!("clear; echo \"{drive} $(cat /volumes/{drive}/hello.txt)\"\n"))?;
        vm.expect_screen(&[&format!("{drive} Read from a disc.")])?;
        drive_says(vm, drive, "8192", "cd-rom data", 1)?;
    }

    // The disc's file goes to the disk on its channel while the other
    // drive reads its own: the disk's writes and the drive's reads take
    // turns on the primary channel, and the secondary's drive has its
    // channel to itself.
    vm.type_text(
        "clear; cat /volumes/cd1/big.bin > /dev/null & cp /volumes/cd0/big.bin /volumes/sda/big.bin; wait; sync; cmp /volumes/cd0/big.bin /volumes/sda/big.bin && cmp /volumes/cd0/big.bin /volumes/cd1/big.bin; echo \"copied $(( $? + 100 ))\"\n",
    )?;
    vm.expect_screen(&["copied 100"])?;

    // The drive is looked at every two seconds: by the second look after
    // the mount, the disc is locked in. QEMU, refused, presses the
    // drive's eject button: volumed unmounts the disc and ejects it.
    std::thread::sleep(Duration::from_secs(5));
    match vm.monitor("eject disk1") {
        Err(why) if why.contains("is locked") => {}
        Err(why) => return Err(why),
        Ok(()) => return Err("QEMU ejected the disc the drive should have locked in".into()),
    }
    vm.expect("Disk: cd0: its eject button was pressed")?;
    vm.expect("Disk: cd0: its disc was ejected")?;
    vm.expect("volumed: the eject button of cd0 was pressed; unmounted /volumes/cd0")?;
    vm.expect("volumed: cd0 has no disc in it now")?;
    drive_says(vm, "cd0", "0", "none", 2)?;

    // Put in again: mounted at its label.
    let image = vm.dir().join("optical-ide-disk1.img");
    vm.monitor(&format!("change disk1 {} raw", image.display()))?;
    vm.expect("Disk: cd0: a disc of data (cd-rom) of 4 MiB went in")?;
    vm.expect("volumed: a disc (cd-rom) went in cd0")?;
    vm.expect("volumed: mounted the ISO 9660 filesystem of cd0 at /volumes/RADIXPRI, read-only")?;
    vm.type_text("clear; cmp /volumes/RADIXPRI/big.bin /volumes/sda/big.bin; echo \"same $(( $? + 100 ))\"\n")?;
    vm.expect_screen(&["same 100"])?;
    drive_says(vm, "cd0", "8192", "cd-rom data", 3)?;

    // Pulled out by force while mounted: lost, and unmounted all the same.
    vm.monitor("eject -f disk1")?;
    vm.expect("Disk: cd0: its disc was taken out")?;
    vm.expect("volumed: cd0 is gone; unmounted /volumes/RADIXPRI")?;
    vm.expect("volumed: cd0 has no disc in it now")?;
    drive_says(vm, "cd0", "0", "none", 4)?;

    // The disk on the channel still answers.
    vm.type_text("clear; echo again > /volumes/sda/again.txt; sync; cat /volumes/sda/again.txt\n")?;
    vm.expect_screen(&["again"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
