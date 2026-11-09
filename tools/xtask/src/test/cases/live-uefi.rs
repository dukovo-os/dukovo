// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install stick, as *ob image* makes it, boots under UEFI from USB
//! into the live system (ADR 0308), with a disk holding a Dukovo root
//! beside it. The boot menu's first entry, *Try Dukovo (live console)*,
//! starts the system from the boot archive with *live* on the command
//! line: Spark takes no disk's root, leaves the other disk unmounted, and
//! mounts the stick's DUKOVO_LIVE partition read-only. Its signed release
//! checks against the root the live system trusts, every file of it, with
//! *system-release check*, and nothing can be written to it.

use super::super::*;

pub const CASE: Case = Case {
    name: "live-uefi",
    about: "The install stick boots under UEFI from USB into the live system: no disk's root is taken, only the stick's DUKOVO_LIVE is mounted, read-only, and its signed release checks whole against the trusted root",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // The live menu entry has its own flags, `live` and
    // `console-sign-in=off`, before these.
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &[],
    script: live,
    disks: ROOT_DISKS,
    second: None,
};

/// The live system starts from the stick, takes no root, and checks the
/// stick's release, then powers off. live-bios boots the same way, and
/// live-dvd from the install DVD.
pub fn live(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: the live system: /system is the boot archive, in memory, and no disk is the root")?;
    vm.expect("spark: built the hierarchy of ")?;
    let log = vm.log();
    let medium = live_medium(&log)?;
    if log.contains("spark: /system is Dukovo's root on ") {
        return Err("Spark took a disk's root in the live system".into());
    }
    if log.contains("spark: mounted the Btrfs filesystem of vda") {
        return Err("Spark mounted the machine's own disk in the live system".into());
    }
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    // The live medium alone is in /volumes.
    vm.type_text("echo \"volumes: $(ls /volumes | wc -l)\"\n")?;
    vm.expect_screen(&["volumes: 1"])?;

    // The signed release checks whole. Its lines are longer than the
    // screen, and the shell shows each command as it is typed, so each
    // answer is a sum the command line does not hold.
    vm.type_text(&format!(
        "system-release check /volumes/{medium}/release > /tmp/check 2>&1; \
         echo \"checked: $((100 + $?))\"\n"
    ))?;
    vm.expect_screen(&["checked: 1"])?;
    if !vm.screen()?.iter().any(|line| line.trim() == "checked: 100") {
        vm.type_text("tail -n 4 /tmp/check\n")?;
        let _ = vm.expect_screen(&["system-release: "]);
        return Err(format!(
            "system-release check failed on the live medium's release: {}",
            vm.screen()?.join(" | ")
        ));
    }
    vm.type_text(&format!(
        "grep -q 'release {VERSION} (' /tmp/check && grep -q 'files match the signed list' \
         /tmp/check; echo \"said: $((100 + $?))\"\n"
    ))?;
    vm.expect_screen(&["said: 100"])?;

    // Nothing writes the release.
    vm.type_text(&format!(
        "touch /volumes/{medium}/release/x 2>/dev/null; echo \"touch: $((40 + $?))\"\n"
    ))?;
    vm.expect_screen(&["touch: 41"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The name of the live medium, the stick's DUKOVO_LIVE partition or the
/// install DVD's drive, from Spark's line `spark: mounted the <kind>
/// filesystem of <name> at /volumes/<name>, read-only`: a Btrfs on the
/// stick, ISO 9660 on the DVD.
fn live_medium(log: &str) -> Result<String, String> {
    log.lines()
        .filter_map(|line| line.split("spark: mounted the ").nth(1))
        .filter_map(|rest| rest.split_once(" filesystem of ").map(|(_, rest)| rest))
        .find(|rest| rest.ends_with(", read-only"))
        .and_then(|rest| rest.split(' ').next())
        .map(str::to_string)
        .ok_or_else(|| "Spark did not mount the live medium read-only".into())
}
