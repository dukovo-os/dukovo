// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A USB card reader with no card in it (QEMU's usb-storage on an empty
//! drive, its medium removable) is a drive of no sectors, sdb, which
//! Spark leaves alone (ADR 0430). A card put in it with the monitor's
//! change command goes in: the kernel finds it within two seconds, and
//! volumed mounts it at its label, where a file is written. While it is
//! mounted the drive locks it in, so that QEMU refuses to eject it.
//! eject(1) unmounts it and has the drive eject it; the drive is empty
//! again, and its count of media changes says so. Put in again, the card
//! holds the file; pulled out by force while mounted, its volume is
//! unmounted though it cannot be written. The host reads the file back
//! from the card's image.

use std::time::Duration;

use super::super::*;

pub const CASE: Case = Case {
    name: "card-reader",
    about: "A USB card reader with no card is an empty drive; a card put in is mounted at its label, locked in while mounted, ejected by eject, and unmounted when pulled out by force; the host reads back what was written",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // No program: Spark and volumed mount, as on a normal boot.
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-drive",
        "if=none,id=card0",
        "-device",
        "usb-storage,drive=card0,bus=xhci.0,port=2,id=reader,removable=on",
    ],
    script: card_reader,
    disks: &[],
    second: None,
};

/// The reader's name: the first `sd` letter after the boot stick's.
const NAME: &str = "sdb";
/// The card's label, where volumed mounts it.
const LABEL: &str = "DUKOVOCARD";
/// What is written on the card.
const TEXT: &str = "written on a card, ejected safely";

/// Makes the card at `path`: 32 MiB of FAT16, whole, labelled [`LABEL`].
fn make_card(path: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(path);
    let path = path.display().to_string();
    let mut mkfs = std::process::Command::new(fat_tool("mkfs.fat"));
    host_tool(mkfs.args(["-C", "-n", LABEL, &path, "32768"]))?;
    Ok(())
}

/// Puts the card at `card` in the reader, and waits for volumed to mount
/// it at its label.
fn put_in(vm: &mut Vm, card: &Path) -> Result<(), String> {
    vm.monitor(&format!("change card0 {} raw", card.display()))?;
    vm.expect(&format!("Disk: {NAME}: a medium of 32 MiB went in, 512 bytes a block"))?;
    vm.expect(&format!("Disk: {NAME} has no partition table; it is used whole"))?;
    vm.expect(&format!("volumed: a medium went in {NAME}"))?;
    vm.expect(&format!("volumed: mounted the FAT filesystem of {NAME} at /volumes/{LABEL}"))
}

/// What `/kernel/block` says of the reader: its size and its count of
/// media changes, on the screen.
fn reader_says(vm: &mut Vm, size: &str, changes: u32) -> Result<(), String> {
    vm.type_text(&format!(
        "clear; echo \"size $(cat /kernel/block/{NAME}/size) removable $(cat /kernel/block/{NAME}/removable) changes $(cat /kernel/block/{NAME}/media_changes)\"\n"
    ))?;
    vm.expect_screen(&[&format!("size {size} removable 1 changes {changes}")])
}

fn card_reader(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect_anywhere(&format!(
        "Disk: {NAME} is a USB drive with no medium in it on 1-2: QEMU QEMU HARDDISK"
    ))?;
    vm.expect_anywhere(&format!("spark: {NAME} has no medium in it; volumed mounts one put in"))?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("volumed: watching for volumes; ")?;
    vm.expect_anywhere("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    reader_says(vm, "0", 0)?;

    let card = vm.dir().join("card-reader-card.img");
    make_card(&card)?;
    put_in(vm, &card)?;
    reader_says(vm, "65536", 1)?;
    vm.type_text(&format!(
        "clear; echo '{TEXT}' > /volumes/{LABEL}/card.txt; sync; echo \"status $(( $? + 100 ))\"\n"
    ))?;
    vm.expect_screen(&["status 100"])?;

    // The drive is looked at every two seconds: by the second look after
    // the mount, the card is locked in.
    std::thread::sleep(Duration::from_secs(5));
    match vm.monitor("eject card0") {
        Err(why) if why.contains("is locked") => {}
        Err(why) => return Err(why),
        Ok(()) => return Err("QEMU ejected the card the drive should have locked in".into()),
    }

    vm.type_text(&format!("clear; eject {LABEL}; echo \"status $(( $? + 100 ))\"\n"))?;
    vm.expect(&format!("Disk: {NAME}: its medium was ejected"))?;
    vm.expect_screen(&["status 100"])?;
    vm.expect(&format!("volumed: {NAME} has no medium in it now"))?;
    reader_says(vm, "0", 2)?;

    put_in(vm, &card)?;
    vm.type_text(&format!("clear; cat /volumes/{LABEL}/card.txt\n"))?;
    vm.expect_screen(&[TEXT])?;

    // Pulled out by force while mounted: lost, and unmounted all the same.
    vm.monitor("eject -f card0")?;
    vm.expect(&format!("Disk: {NAME}: its medium was taken out"))?;
    vm.expect(&format!("volumed: {NAME} is gone; unmounted /volumes/{LABEL}"))?;
    vm.expect(&format!("volumed: {NAME} has no medium in it now"))?;
    reader_says(vm, "0", 4)?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)?;
    let image = card.display().to_string();
    let read =
        host_tool(std::process::Command::new("mtype").args(["-i", &image, "::/card.txt"]))?;
    if read != format!("{TEXT}\n").as_bytes() {
        return Err(format!(
            "the card's card.txt holds {:?}, not what was written",
            String::from_utf8_lossy(&read)
        ));
    }
    Ok(())
}
