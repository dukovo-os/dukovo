// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Firmware sets are updates of their own (ADR 0468). *sysupdate
//! firmware stage* puts a signed test set's microcode in the root's
//! *@firmware* store and stages slot B as a snapshot of slot A that
//! names it. At the restart GRUB loads the set's microcode from the
//! store, which the kernel reports in */kernel/firmware/microcode*, and
//! the health check makes the slot the default, keeping the devices of
//! the start: the root's partition and a second network card. A newer set
//! is staged and the machine powered off; it comes back without the
//! card. The newer set's try finds the card missing and falls back to
//! slot B, with the set before it, GRUB saying the firmware update
//! failed, and slot B, no try, only logs the card. Then a set whose
//! microcode is damaged is staged: the kernel uses none of it, and its
//! try falls back too. The store keeps only the sets a slot names.

use super::super::*;
use super::update_stage::{UPDATE_DISKS, UPDATES, power_off, restart, run, started};

pub const CASE: Case = Case {
    name: "firmware-fallback",
    about: "a firmware set staged as a slot starts its microcode from the root's store and proves itself; a newer set whose try misses a network card the last good start had, and one whose microcode is damaged, fall back to the set before them (ADR 0468)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[FIRST_BOOT_ONLY, "-netdev", "user,id=spare", "-device", SPARE_CARD],
    script: first_boot,
    disks: UPDATE_DISKS,
    second: Some(SecondBoot { cmdline: AS_IT_WAS, before: nothing, script: second_boot }),
};

/// The card the first boot has and the second lacks.
const SPARE_CARD: &str = "e1000,netdev=spare,mac=52:54:00:15:0c:01";

/// Its MAC address, as the device record and the health check say it.
const SPARE_MAC: &str = "52:54:00:15:0c:01";

/// The size of the test sets' microcode, as the kernel reports it: two
/// updates of 1 KiB.
const MICROCODE_SIZE: &str = "size 2048";

fn nothing(_case: &Case, _disks: &[DiskImage]) -> Result<(), String> {
    Ok(())
}

/// Stages the test firmware set `name` into `slot`, `from` what the
/// running slot names.
fn stage(vm: &mut Vm, name: &str, slot: char, from: &str) -> Result<(), String> {
    let version = test_firmware_version(name);
    run(
        vm,
        &format!("sysupdate firmware stage {UPDATES}/{TEST_FIRMWARE_REPO} --version {version}"),
        // The line is wider than the screen: its first part.
        &[&format!(
            "sysupdate: staged firmware {version} in slot {slot}: it starts at the next restart, \
             with 3 tries to prove itself, and slot {}, with {from}, starts again",
            if slot == 'A' { 'B' } else { 'A' }
        )],
    )
}

/// The first boot: the good set proves itself, and the newer one is
/// staged.
fn first_boot(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let good = test_firmware_version("good");
    started(vm, 'a')?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vda at {UPDATES}"))?;
    run(
        vm,
        "printf 'stable = 10\\n' > /system/settings/sysupdate/health.toml && sysupdate firmware",
        &["Slot A:   no firmware set, running", "Firmware package: not taken"],
    )?;
    stage(vm, "good", 'B', "no firmware set")?;
    run(
        vm,
        "sysupdate firmware",
        &[
            &format!("Slot B:   firmware {good}"),
            "intel-ucode.bin (CPU microcode, 2 KiB, LicenseRef-Test)",
        ],
    )?;

    // The try, from the hidden menu: GRUB loads the set's microcode.
    restart(vm)?;
    started(vm, 'b')?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {VERSION}, firmware {good}) proved itself: it is now the \
         default, and slot A (Dukovo {VERSION}) the previous version"
    ))?;
    run(
        vm,
        "sysupdate status; cat /kernel/firmware/microcode",
        &[
            &format!("Running:  slot B, Dukovo {VERSION}, firmware {good}"),
            &format!("Slot A:   Dukovo {VERSION}, the previous version"),
            &format!("Last update: no firmware set to firmware {good} in slot B"),
            MICROCODE_SIZE,
        ],
    )?;
    run(
        vm,
        "sysupdate firmware; cat /system/state/sysupdate/devices",
        &[
            &format!("Slot B:   firmware {good}, running"),
            "Firmware package: taken",
            SPARE_MAC,
            // A tab splits the fields: the screen shows it as spaces.
            "root    ",
        ],
    )?;
    stage(vm, "newer", 'A', &format!("firmware {good}"))?;
    // QEMU writes the boot image to a snapshot it drops when it ends: what
    // is synced now is written to the image, for the second boot.
    run(vm, "sync && echo \"synced: $((4 * 4))\"", &["synced: 16"])?;
    vm.monitor("commit boot")?;
    power_off(vm)
}

/// The second boot, without the spare card: the newer set's try misses
/// it, and the broken set's microcode is damaged.
fn second_boot(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let (good, newer, broken) = (
        test_firmware_version("good"),
        test_firmware_version("newer"),
        test_firmware_version("broken"),
    );
    vm.expect("spark: started slot a, @system.a")?;
    let named = format!("slot A (Dukovo {VERSION}, firmware {newer})");
    verdict(vm, &named, "failed its health check: the network card ")?;
    vm.expect(&format!("({SPARE_MAC}), there at the last good start, is missing"))?;
    vm.expect(&format!(
        "sysupdate: the update gives up its tries; restarting into slot B (Dukovo {VERSION}, \
         firmware {good})"
    ))?;
    vm.expect(&format!("Booting `Dukovo Server {VERSION} (the firmware update to {newer} failed)'"))?;
    vm.expect("spark: the update in slot a fell back: putting the settings from before it back")?;
    // The health check and tty1 come in either order.
    vm.expect("spark: started slot b, @system.b")?;
    vm.expect_ahead(&format!(
        "({SPARE_MAC}), there at the last good start, is missing; as this start is no update's \
         try, it goes on"
    ))?;
    vm.expect_ahead(&format!(
        "sysupdate: the update in slot A (Dukovo {VERSION}, firmware {newer}) did not prove \
         itself; slot B (Dukovo {VERSION}, firmware {good}) runs, with the settings from before \
         the update"
    ))?;
    vm.expect("; tty1 is shown")?;

    // A set whose microcode is damaged: the kernel uses none of it.
    stage(vm, "broken", 'A', &format!("firmware {good}"))?;
    restart(vm)?;
    vm.expect("microcode: module of 2048 bytes with ")?;
    vm.expect("data that failed its checks; none of it is used")?;
    vm.expect("spark: started slot a, @system.a")?;
    let named = format!("slot A (Dukovo {VERSION}, firmware {broken})");
    verdict(
        vm,
        &named,
        &format!(
            "failed its health check: the CPU microcode of firmware {broken} has data that \
             failed its checks, so none of it was used"
        ),
    )?;
    vm.expect(&format!("Booting `Dukovo Server {VERSION} (the firmware update to {broken} failed)'"))?;
    started(vm, 'b')?;
    run(
        vm,
        "sysupdate firmware; ls /volumes/*/@firmware/sets",
        &[
            &format!("Slot A:   firmware {broken}"),
            &format!("Slot B:   firmware {good}, running"),
            &format!("{good}.json"),
            &format!("{broken}.json"),
        ],
    )?;
    run(
        vm,
        &format!(
            "[ -e /volumes/*/@firmware/sets/{newer}.json ] || echo \"collected: $((5 * 9))\""
        ),
        &["collected: 45"],
    )?;
    power_off(vm)
}

/// Follows the health check's try of the slot `named` to its verdict,
/// which must start with `expected`: on another, says what it was.
fn verdict(vm: &mut Vm, named: &str, expected: &str) -> Result<(), String> {
    vm.expect(&format!("sysupdate: {named} is an update being tried"))?;
    vm.expect(&format!("sysupdate: {named} "))?;
    let log = vm.log();
    let line = log.get(vm.cursor..).and_then(|rest| rest.lines().next()).unwrap_or_default();
    if line.starts_with(expected) {
        Ok(())
    } else {
        Err(format!("the health check said of {named}: {}", line.trim_end()))
    }
}
