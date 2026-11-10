// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Two blank floppy disks, a 1.44 MB one and a 720 KB one, in two drives
//! on QEMU's ISA floppy disk controller (ADR 0436). `floppy` lists the
//! drives, the sizes they format and what is in them. `floppy format`
//! formats each disk a track at a time through radix-block(2)'s
//! FORMAT_TRACK, has the kernel look, reads the disk back, makes a FAT
//! filesystem labelled as asked, and mounts it at its label; a file
//! written there reads back. The kernel refuses to format a disk that is
//! mounted, and one that is write-protected. The host checks both
//! filesystems with `fsck.fat` and reads back the file and labels.

use std::fmt::Write as _;

use super::super::*;

pub const CASE: Case = Case {
    name: "floppy-format",
    about: "floppy lists two floppy drives with blank disks in them, and floppy format formats each, a 1.44 MB and a 720 KB disk, makes a FAT filesystem labelled as asked and mounts it; a file written there reads back, and a disk mounted or write-protected is not formatted",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: floppy_format,
    disks: DISKS,
    second: None,
};

/// A blank 1.44 MB disk in drive 0 (fd0) and a blank 720 KB one in drive
/// 1 (fd1), zeros only: QEMU's drive formats nothing, and reads any image
/// as formatted.
const DISKS: &[Disk] = &[
    Disk { fat: BLANK, check: formatted, ..fat_disk(1440, 12, "LABFMT", Table::None, Bus::Floppy) },
    Disk { fat: BLANK, check: formatted, ..fat_disk(720, 12, "DD720", Table::None, Bus::Floppy) },
];

/// The lines of the file written to the 1.44 MB disk.
const LINES: u32 = 20_000;

/// The host's check of a disk once the case is done: a FAT filesystem
/// `fsck.fat` finds whole, with the label it was made with.
fn formatted(disk: &DiskImage, log: &str) -> Result<(), String> {
    fsck(disk, log)?;
    let image = disk.path.display().to_string();
    let label = host_tool(std::process::Command::new("mlabel").args(["-i", &image, "-s", "::"]))?;
    let label = String::from_utf8_lossy(&label);
    let want = if image.ends_with("disk0.img") { "LABFMT" } else { "DD720" };
    if !label.contains(want) {
        return Err(format!("{image} is labelled \"{}\", not {want}", label.trim()));
    }
    Ok(())
}

/// Runs `command`, and waits for the line saying how it ended: `ended
/// 100` if it worked.
fn run(vm: &mut Vm, command: &str, ended: u32) -> Result<(), String> {
    vm.type_text(&format!("clear; {command}; echo \"ended $(( $? + 100 ))\"\n"))?;
    vm.expect_screen(&[&format!("ended {ended}")])
}

fn floppy_format(vm: &mut Vm, case: &Case) -> Result<(), String> {
    disks_found(vm, case)?;
    vm.expect_anywhere("volumed: watching for volumes; ")?;
    vm.expect_anywhere("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    // The drives, and the sizes of disk they format.
    vm.type_text("clear; floppy; cat /kernel/block/fd0/formats\n")?;
    vm.expect_screen(&[
        "fd0: formats 1.44M and 720K; a 1.44M disk",
        "fd1: formats 1.44M and 720K; a 720K disk",
        "1.44M 720K",
    ])?;

    // Formatted, looked at, read back, given a FAT and mounted at its
    // label.
    run(vm, "floppy format -n LABFMT fd0", 100)?;
    vm.expect("Disk: fd0: its disk is being formatted")?;
    vm.expect("Disk: fd0: formatting its disk as a 1.44 MB one")?;
    vm.expect("Disk: fd0: a 1.44 MB disk went in")?;
    vm.expect_screen(&[
        "floppy: reading the disk back",
        "floppy: making a FAT filesystem on it",
        "floppy: mounted the FAT filesystem of fd0 at /volumes/LABFMT",
    ])?;
    let write = format!(
        "seq 1 {LINES} > /volumes/LABFMT/seq.txt && seq 1 {LINES} | cmp - /volumes/LABFMT/seq.txt"
    );
    run(vm, &write, 100)?;

    // Not while it is mounted.
    run(vm, "floppy format fd0", 101)?;
    vm.expect_screen(&["fd0 is mounted at /volumes/LABFMT", "(eject it first)"])?;

    // A 720 KB disk, at its own size.
    run(vm, "floppy format -s 720K -n DD720 fd1", 100)?;
    vm.expect("Disk: fd1: formatting its disk as a 720 KB one")?;
    vm.expect("Disk: fd1: a 720 KB disk went in")?;
    vm.expect_screen(&["floppy: mounted the FAT filesystem of fd1 at /volumes/DD720"])?;

    // Let go and put back write-protected: the kernel refuses to format
    // it.
    run(vm, "eject fd1", 100)?;
    vm.expect("Disk: fd1: its disk was let go, and may be taken out")?;
    let image = vm.dir().join("floppy-format-disk1.img");
    vm.monitor(&format!("change disk1 {} raw read-only", image.display()))?;
    run(vm, "floppy format -s 720K fd1", 101)?;
    vm.expect_screen(&["fd1: cylinder 0, head 0: the disk is write-protected"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)?;
    let want = (1..=LINES).fold(String::new(), |mut text, n| {
        let _ = writeln!(text, "{n}");
        text
    });
    let image = vm.dir().join("floppy-format-disk0.img").display().to_string();
    let read = host_tool(std::process::Command::new("mtype").args(["-i", &image, "::/seq.txt"]))?;
    if read != want.as_bytes() {
        return Err("the formatted disk's seq.txt does not hold what was written".into());
    }
    Ok(())
}
