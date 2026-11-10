// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Two 3.5-inch 1.44 MB floppy drives on QEMU's ISA floppy disk
//! controller (ADR 0435), with a 1.44 MB disk in the first and a 720 KB
//! one in the second at boot. The kernel finds the controller in the
//! ACPI namespace, names the drives fd0 and fd1, tells the disks apart
//! by their data rates, and Spark mounts each at the drive's name. Files
//! written across many cylinders of both disks read back the same. The
//! drives are never polled: eject(1) lets the first disk go, and a look
//! asked with `rdisk --reread` leaves the drive empty while the disk is
//! still in it. Taken out and put in again, the disk is found at the next
//! look and mounted at its label, its files read from it; taken out while
//! mounted, the next read finds its drive's change line up, and its
//! volume is unmounted. The second disk, swapped for itself
//! write-protected, is found so at a look and mounted read-only. The host
//! reads back what was written from the disks' images.

use std::fmt::Write as _;

use super::super::*;

pub const CASE: Case = Case {
    name: "floppy",
    about: "A 1.44 MB and a 720 KB disk in two floppy drives are found at boot, mounted, written and read back; the drives are looked at only when asked: a disk let go stays out, one put in again is mounted at its label, one taken out while mounted is lost at the next read, and a write-protected one is mounted read-only",
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
    script: floppy,
    disks: DISKS,
    second: None,
};

/// A 1.44 MB disk in drive 0 (fd0, QEMU's disk0) and a 720 KB one in
/// drive 1 (fd1, disk1), each FAT12 on the whole disk, as `mkfs.fat`
/// makes a floppy disk.
const DISKS: &[Disk] = &[
    fat_disk(1440, 12, "RADIXFD", Table::None, Bus::Floppy),
    fat_disk(720, 12, "RADIX720", Table::None, Bus::Floppy),
];

/// The lines of the file written: about 230 KB, thirteen cylinders of a
/// 1.44 MB disk and 25 of a 720 KB one.
const LINES: u32 = 40_000;

/// What `/kernel/block` says of `drive`, on the screen: its size in
/// sectors, that it is removable, and its count of media changes.
fn drive_says(vm: &mut Vm, drive: &str, size: &str, changes: u32) -> Result<(), String> {
    let at = format!("/kernel/block/{drive}");
    vm.type_text(&format!(
        "clear; echo \"{drive} size $(cat {at}/size) removable $(cat {at}/removable) changes $(cat {at}/media_changes)\"\n"
    ))?;
    vm.expect_screen(&[&format!("{drive} size {size} removable 1 changes {changes}")])
}

/// Has the kernel look in `drive` (radix-block(2)'s REREAD_PARTITIONS),
/// and waits for the command to end.
fn look(vm: &mut Vm, drive: &str) -> Result<(), String> {
    vm.type_text(&format!(
        "clear; rdisk --reread /devices/{drive} > /dev/null; echo \"looked $(( $? + 100 ))\"\n"
    ))?;
    vm.expect_screen(&["looked 100"])
}

fn floppy(vm: &mut Vm, case: &Case) -> Result<(), String> {
    disks_found(vm, case)?;
    vm.expect_anywhere("Disk: a floppy disk controller at port 0x3f0: an 82077AA")?;
    for (disk, name) in case.disks.iter().zip(["fd0", "fd1"]) {
        let label = disk.label;
        vm.expect_anywhere(&format!(
            "Files: mounted fat from {name} on /volumes/{name}, labelled {label}"
        ))?;
        vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of {name} at /volumes/{name}"))?;
    }
    vm.expect_anywhere("volumed: watching for volumes; ")?;
    vm.expect_anywhere("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    drive_says(vm, "fd0", "2880", 1)?;
    drive_says(vm, "fd1", "1440", 1)?;

    // Written across cylinders of both disks, a cylinder at a time.
    vm.type_text(&format!(
        "clear; seq 1 {LINES} > /volumes/fd0/seq.txt && seq 1 9000 > /volumes/fd0/later.txt && cp /volumes/fd0/seq.txt /volumes/fd1/seq.txt && sync; echo \"wrote $(( $? + 100 ))\"\n"
    ))?;
    vm.expect_screen(&["wrote 100"])?;

    // Let go: the drive is empty, and a look leaves it so while the disk
    // is still in it.
    vm.type_text("clear; eject fd0; echo \"ejected $(( $? + 100 ))\"\n")?;
    vm.expect("Disk: fd0: its disk was let go, and may be taken out")?;
    vm.expect_screen(&["ejected 100"])?;
    vm.expect("volumed: fd0 has no medium in it now")?;
    look(vm, "fd0")?;
    drive_says(vm, "fd0", "0", 2)?;

    // Taken out and put in again: found at the next look, and mounted at
    // its label, its file read from it.
    let image = vm.dir().join("floppy-disk0.img");
    vm.monitor("eject disk0")?;
    vm.monitor(&format!("change disk0 {} raw", image.display()))?;
    look(vm, "fd0")?;
    vm.expect("Disk: fd0: a 1.44 MB disk went in")?;
    vm.expect("volumed: a medium went in fd0")?;
    vm.expect("volumed: mounted the FAT filesystem of fd0 at /volumes/RADIXFD")?;
    vm.type_text(&format!(
        "clear; seq 1 {LINES} | cmp - /volumes/RADIXFD/seq.txt; echo \"same $(( $? + 100 ))\"\n"
    ))?;
    vm.expect_screen(&["same 100"])?;
    drive_says(vm, "fd0", "2880", 3)?;

    // Taken out while mounted: the next read finds the change line up.
    vm.monitor("eject disk0")?;
    vm.type_text("clear; cat /volumes/RADIXFD/later.txt > /dev/null\n")?;
    vm.expect("Disk: fd0: its disk was taken out")?;
    vm.expect("volumed: fd0 is gone; unmounted /volumes/RADIXFD")?;
    vm.expect("volumed: fd0 has no medium in it now")?;
    drive_says(vm, "fd0", "0", 4)?;

    // Swapped for itself, write-protected: found so, and mounted
    // read-only, the 720 KB disk read at its own data rate.
    let image = vm.dir().join("floppy-disk1.img");
    vm.monitor(&format!("change disk1 {} raw read-only", image.display()))?;
    look(vm, "fd1")?;
    vm.expect("Disk: fd1: its disk was taken out")?;
    vm.expect("Disk: fd1: a 720 KB disk went in, write-protected")?;
    vm.expect("volumed: mounted the FAT filesystem of fd1 at /volumes/RADIX720, read-only")?;
    // volumed may see the disk go while the look finds the new one.
    vm.expect_anywhere("volumed: fd1 is gone; unmounted /volumes/fd1")?;
    vm.type_text(&format!(
        "clear; seq 1 {LINES} | cmp - /volumes/RADIX720/seq.txt; echo \"same $(( $? + 100 ))\"\n"
    ))?;
    vm.expect_screen(&["same 100"])?;
    vm.type_text(
        "clear; touch /volumes/RADIX720/no.txt 2> /dev/null; echo \"refused $(( $? > 0 ))\"\n",
    )?;
    vm.expect_screen(&["refused 1"])?;
    drive_says(vm, "fd1", "1440", 3)?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)?;
    let want = (1..=LINES).fold(String::new(), |mut text, n| {
        let _ = writeln!(text, "{n}");
        text
    });
    for name in ["floppy-disk0.img", "floppy-disk1.img"] {
        let image = vm.dir().join(name).display().to_string();
        let read =
            host_tool(std::process::Command::new("mtype").args(["-i", &image, "::/seq.txt"]))?;
        if read != want.as_bytes() {
            return Err(format!("{name}'s seq.txt does not hold what was written"));
        }
    }
    Ok(())
}
