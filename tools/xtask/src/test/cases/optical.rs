// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Three optical drives, each with a disc in it at boot (ADR 0431): a DVD
//! drive on an AHCI port (QEMU's `ide-cd`, which takes its commands in
//! ATA packets) holding an ISO 9660 disc with Rock Ridge and Joliet, and
//! two USB drives (`scsi-cd`), one over USB Attached SCSI holding a UDF
//! disc with the ISO 9660 bridge a DVD has, one over Bulk-Only holding
//! another ISO 9660 disc. The kernel names them cd0, cd1 and cd2, reads
//! each disc whole and read-only, and Spark mounts each at the drive's
//! name in `/volumes`, where a file reads as the host made it. lsblk says
//! each drive is a `rom`, and `/kernel/block/<drive>/disc` what is in it.
//! While mounted, a disc is locked in, so that QEMU refuses to eject it
//! and presses the drive's eject button instead (ADR 0432): the drive
//! says so, and volumed unmounts the disc and ejects it, on AHCI as on
//! USB. A disc put in again is mounted by volumed at its label; eject(1)
//! unmounts it and opens the drive's tray, and the drive says it is
//! empty; one pulled out by force while mounted is unmounted all the
//! same. The USB drive over UAS ejects its disc too. No disc changes.

use std::time::Duration;

use super::super::*;

pub const CASE: Case = Case {
    name: "optical",
    about: "Discs in an AHCI DVD drive and two USB optical drives are read whole and read-only and mounted at boot; a disc is locked in while mounted, ejected by the drive's eject button and by eject, mounted at its label when put in again, and unmounted when pulled out by force",
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
    script: optical,
    disks: DISKS,
    second: None,
};

/// The discs: in the AHCI drive (cd0), the USB drive over UAS at QEMU's
/// port 2 (cd1) and the one over Bulk-Only at port 3 (cd2).
const DISKS: &[Disk] = &[
    disc(ISO9660, "RADIXISO", Bus::SataCd),
    disc(UDF, "RADIXUDF", Bus::UasCd("2", 0)),
    disc(ISO9660, "RADIXBOT", Bus::UsbCd("3")),
];

/// The drives' names, in the order of [`DISKS`].
const DRIVES: [&str; 3] = ["cd0", "cd1", "cd2"];

/// A disc image of 4 MiB, which QEMU says is a CD-ROM.
const fn disc(fat: u8, label: &'static str, bus: Bus) -> Disk {
    Disk {
        kib: 4096,
        fat,
        cluster: 0,
        label,
        table: Table::None,
        bus,
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

/// Presses the eject button of the drive `drive`, QEMU's `id`, whose
/// disc is mounted and locked in: QEMU refuses to eject it and asks the
/// guest, the drive says the button was pressed, and volumed unmounts the
/// disc and ejects it, which the kernel says as `ejected` (ADR 0432).
fn eject_button(vm: &mut Vm, id: &str, drive: &str, ejected: &str) -> Result<(), String> {
    match vm.monitor(&format!("eject {id}")) {
        Err(why) if why.contains("is locked") => {}
        Err(why) => return Err(why),
        Ok(()) => return Err("QEMU ejected the disc the drive should have locked in".into()),
    }
    vm.expect(&format!("Disk: {drive}: its eject button was pressed"))?;
    vm.expect(&format!("Disk: {drive}: {ejected}"))?;
    vm.expect(&format!(
        "volumed: the eject button of {drive} was pressed; unmounted /volumes/{drive}"
    ))?;
    vm.expect(&format!("volumed: {drive} has no disc in it now"))
}

fn optical(vm: &mut Vm, case: &Case) -> Result<(), String> {
    disks_found(vm, case)?;
    for (disk, name) in case.disks.iter().zip(DRIVES) {
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

    // Every disc reads as the host made it, and its drive says what it
    // is. The AHCI drive was found empty, and its disc went in at its
    // first look; a USB drive was found with its disc.
    for drive in DRIVES {
        vm.type_text(&format!("clear; echo \"{drive} $(cat /volumes/{drive}/hello.txt)\"\n"))?;
        vm.expect_screen(&[&format!("{drive} Read from a disc.")])?;
        let changes = if drive == "cd0" { 1 } else { 0 };
        drive_says(vm, drive, "8192", "cd-rom data", changes)?;
    }

    // The drive is looked at every two seconds: by the second look after
    // the mount, the disc is locked in. QEMU, refused, presses the
    // drive's eject button: volumed unmounts the disc and ejects it.
    std::thread::sleep(Duration::from_secs(5));
    eject_button(vm, "disk0", "cd0", "its disc was ejected")?;
    drive_says(vm, "cd0", "0", "none", 2)?;

    // Put in again: mounted at its label.
    let image = vm.dir().join("optical-disk0.img");
    let put_in = |vm: &mut Vm| -> Result<(), String> {
        vm.monitor(&format!("change disk0 {} raw", image.display()))?;
        vm.expect("Disk: cd0: a disc of data (cd-rom) of 4 MiB went in")?;
        vm.expect("volumed: a disc (cd-rom) went in cd0")?;
        vm.expect(
            "volumed: mounted the ISO 9660 filesystem of cd0 at /volumes/RADIXISO, read-only",
        )?;
        vm.type_text("clear; cat /volumes/RADIXISO/hello.txt\n")?;
        vm.expect_screen(&["Read from a disc."])
    };
    put_in(vm)?;
    drive_says(vm, "cd0", "8192", "cd-rom data", 3)?;

    // eject(1) unmounts it and opens the tray.
    vm.type_text("clear; eject RADIXISO; echo \"status $(( $? + 100 ))\"\n")?;
    vm.expect("Disk: cd0: its disc was ejected")?;
    vm.expect_screen(&["status 100"])?;
    vm.expect("volumed: cd0 has no disc in it now")?;
    drive_says(vm, "cd0", "0", "none", 4)?;

    // Pulled out by force while mounted: lost, and unmounted all the same.
    put_in(vm)?;
    vm.monitor("eject -f disk0")?;
    vm.expect("Disk: cd0: its disc was taken out")?;
    vm.expect("volumed: cd0 is gone; unmounted /volumes/RADIXISO")?;
    vm.expect("volumed: cd0 has no disc in it now")?;
    drive_says(vm, "cd0", "0", "none", 6)?;

    // The USB drive over Bulk-Only has its eject button too.
    eject_button(vm, "disk2", "cd2", "its medium was ejected")?;
    drive_says(vm, "cd2", "0", "none", 1)?;

    // A USB drive's disc is ejected the same way.
    vm.type_text("clear; eject cd1; echo \"status $(( $? + 100 ))\"\n")?;
    vm.expect("Disk: cd1: its medium was ejected")?;
    vm.expect_screen(&["status 100"])?;
    vm.expect("volumed: cd1 has no disc in it now")?;
    drive_says(vm, "cd1", "0", "none", 1)?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
