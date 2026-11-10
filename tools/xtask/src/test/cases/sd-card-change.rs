// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An SD card reader with no card in it (QEMU's `sdhci-pci`, holding
//! an `sd-card` on an empty drive) is a slot of no sectors, mmcblk0,
//! which Spark leaves alone (ADR 0437). A card put in with the monitor's change
//! command is found by the slot's card detect within two seconds, set up,
//! and mounted by volumed at its label, where a file is written. eject(1)
//! unmounts it and has the kernel let it go: the slot is empty and stays
//! so while the card is in, until it is taken out and put in again, when
//! it holds the file. Swapped for another card between two looks, the
//! first is taken out and the second put in; pulled out while mounted, the
//! second is unmounted though it cannot be written. The host reads the
//! file back from the first card's image.

use std::time::Duration;

use super::super::*;

pub const CASE: Case = Case {
    name: "sd-card-change",
    about: "An SD card reader with no card is an empty slot; a card put in is set up and mounted at its label, let go by eject until it is taken out, put in again with its file, swapped for another, and unmounted when pulled out; the host reads back what was written",
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
        "sdhci-pci,id=reader",
        "-device",
        "sd-card,drive=card0,bus=pcie.0/reader/sd-bus",
    ],
    script: sd_card_change,
    disks: &[],
    second: None,
};

/// The slot's name: the first SD card reader's first slot.
const NAME: &str = "mmcblk0";
/// The cards' labels, where volumed mounts them.
const LABEL: &str = "DUKOVOSD";
const OTHER: &str = "OTHERSD";
/// What is written on the first card.
const TEXT: &str = "written on an SD card, let go safely";

/// Makes the card at `path`: `kib` KiB of FAT16, whole, labelled `label`.
/// QEMU's cards are a power of two in size.
fn make_card(path: &Path, kib: u32, label: &str) -> Result<(), String> {
    let _ = std::fs::remove_file(path);
    let path = path.display().to_string();
    let mut mkfs = std::process::Command::new(fat_tool("mkfs.fat"));
    host_tool(mkfs.args(["-C", "-n", label, &path, &kib.to_string()]))?;
    Ok(())
}

/// Waits for the card of `size` labelled `label` to be set up in the slot
/// and mounted at its label.
fn went_in(vm: &mut Vm, size: &str, label: &str) -> Result<(), String> {
    vm.expect(&format!("Disk: {NAME}: an SD card of {size} went in: QEMU!, High Speed at 26 MHz on a 4-bit bus"))?;
    vm.expect(&format!("Disk: {NAME} has no partition table; it is used whole"))?;
    vm.expect(&format!("volumed: a medium went in {NAME}"))?;
    vm.expect(&format!("volumed: mounted the FAT filesystem of {NAME} at /volumes/{label}"))
}

/// What `/kernel/block` says of the slot: its size and its count of media
/// changes, on the screen.
fn slot_says(vm: &mut Vm, size: &str, changes: u32) -> Result<(), String> {
    vm.type_text(&format!(
        "clear; echo \"size $(cat /kernel/block/{NAME}/size) removable $(cat /kernel/block/{NAME}/removable) changes $(cat /kernel/block/{NAME}/media_changes)\"\n"
    ))?;
    vm.expect_screen(&[&format!("size {size} removable 1 changes {changes}")])
}

fn sd_card_change(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect_anywhere(&format!("Disk: {NAME} is an empty slot of the SD card reader at "))?;
    vm.expect_anywhere(&format!("spark: {NAME} has no medium in it; volumed mounts one put in"))?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("volumed: watching for volumes; ")?;
    vm.expect_anywhere("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    slot_says(vm, "0", 0)?;

    // Put in: set up, and mounted at its label.
    let card = vm.dir().join("sd-card-change-card.img");
    make_card(&card, 65_536, LABEL)?;
    vm.monitor(&format!("change card0 {} raw", card.display()))?;
    went_in(vm, "64 MiB", LABEL)?;
    slot_says(vm, "131072", 1)?;
    vm.type_text(&format!(
        "clear; echo '{TEXT}' > /volumes/{LABEL}/card.txt; sync; echo \"status $(( $? + 100 ))\"\n"
    ))?;
    vm.expect_screen(&["status 100"])?;

    // Let go: the slot is empty, and stays so while the card is in.
    vm.type_text(&format!("clear; eject {LABEL}; echo \"status $(( $? + 100 ))\"\n"))?;
    vm.expect(&format!("Disk: {NAME}: its card was let go, and may be taken out"))?;
    vm.expect_screen(&["status 100"])?;
    vm.expect(&format!("volumed: {NAME} has no medium in it now"))?;
    std::thread::sleep(Duration::from_secs(5));
    slot_says(vm, "0", 2)?;

    // Taken out and put in again: it holds the file.
    vm.monitor("eject card0")?;
    std::thread::sleep(Duration::from_secs(3));
    vm.monitor(&format!("change card0 {} raw", card.display()))?;
    went_in(vm, "64 MiB", LABEL)?;
    vm.type_text(&format!("clear; cat /volumes/{LABEL}/card.txt\n"))?;
    vm.expect_screen(&[TEXT])?;

    // Swapped for another between two looks: the first is taken out, and
    // the second put in.
    let other = vm.dir().join("sd-card-change-other.img");
    make_card(&other, 32_768, OTHER)?;
    vm.monitor(&format!("change card0 {} raw", other.display()))?;
    vm.expect(&format!("Disk: {NAME}: its card was taken out"))?;
    vm.expect(&format!("volumed: {NAME} is gone; unmounted /volumes/{LABEL}"))?;
    went_in(vm, "32 MiB", OTHER)?;

    // Pulled out while mounted: lost, and unmounted all the same.
    vm.monitor("eject card0")?;
    vm.expect(&format!("Disk: {NAME}: its card was taken out"))?;
    vm.expect(&format!("volumed: {NAME} is gone; unmounted /volumes/{OTHER}"))?;
    vm.expect(&format!("volumed: {NAME} has no medium in it now"))?;
    slot_says(vm, "0", 6)?;

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
