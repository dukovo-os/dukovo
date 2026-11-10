// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An installed system's disk, as *ob image --installed* makes it, boots
//! under UEFI from a SATA disk (ADR 0307). GRUB finds the Dukovo root by
//! its Btrfs UUID and loads the kernel and the startup archive, which
//! holds Spark alone, from the system's slot, *@system.a/startup*. Spark
//! mounts the root the command line names on */system* and starts the
//! services from it. Under KVM, the console service shows tty1 within
//! 10 seconds of GRUB handing over, by the kernel's clock. Idle, with the
//! services running, the machine uses less than 100 MiB, counting the
//! startup archive the boot loader keeps in memory, which must be under
//! a MiB. Both are budgets CI holds every change to (ADR 0427).

use super::super::*;

pub const CASE: Case = Case {
    name: "installed-uefi",
    about: "An installed system's disk boots under UEFI: GRUB loads the kernel and the startup archive (Spark alone, under a MiB) from the root's slot, Spark runs the system from that root, tty1 shows within 10 s under KVM, and the idle machine uses under 100 MiB counting the archive (ADR 0427)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "512M",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: installed,
    disks: &[],
    second: None,
};

/// The most memory an idle installed system may use, in KiB: the budget of
/// the plan's Phase 14e, counting the archive the boot loader keeps.
const IDLE_BUDGET_KIB: u64 = 100 * 1024;

/// The largest startup archive, in KiB: Spark and its mount points.
const STARTUP_ARCHIVE_KIB: u64 = 1024;

/// The longest the boot may take under KVM, in seconds of the kernel's
/// clock, from GRUB handing over to the console service showing tty1,
/// where the sign-in is (ADR 0427). Without KVM it is only reported.
const BOOT_BUDGET_S: f64 = 10.0;

/// The console service's line once tty1 shows.
const TTY1_SHOWN: &str = "; tty1 is shown";

/// An installed system starts from its slot and runs from its root, within
/// its memory budget, then powers off. installed-bios boots the same way.
pub fn installed(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("Init: the boot archive is ")?;
    // The rest of the line may not have come yet when its start has.
    vm.expect(" entries; starting ")?;
    let archive = archive_kib(&vm.log())?;
    if archive > STARTUP_ARCHIVE_KIB {
        return Err(format!(
            "the boot loader gave a boot archive of {archive} KiB, not the startup archive of \
             under {STARTUP_ARCHIVE_KIB} KiB that holds Spark alone"
        ));
    }
    vm.expect("spark: /system is Dukovo's root on ")?;
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect(TTY1_SHOWN)?;
    let shown = log_seconds(&vm.log(), TTY1_SHOWN)?;
    eprintln!(
        "          GRUB to tty1: {shown:.2} s{}",
        if vm.kvm { "" } else { " (no KVM, so not held to the budget)" }
    );
    if vm.kvm && shown > BOOT_BUDGET_S {
        return Err(format!(
            "tty1 showed {shown:.2} s after GRUB handed over, not within {BOOT_BUDGET_S} s"
        ));
    }
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    // The kernel and the archive GRUB loaded are the slot's own files.
    vm.type_text("ls /system/startup && echo \"slot: $((40 + 2))\"\n")?;
    vm.expect_screen(&["archive", "radix", "slot: 42"])?;

    // Once the services have settled: what is neither free nor cache. The
    // label is split in the command so that only the answer shows it whole.
    vm.type_text(
        "sleep 5; awk '/^total:/ { t = $2 } /^free:/ { f = $2 } /^page_cache:/ { c = $2 } \
         END { print \"idle\" \"-used: \" t - f - c }' /processes/meminfo\n",
    )?;
    vm.expect_screen(&["idle-used: "])?;
    let used = idle_used_kib(&vm.screen()?)?;
    let total = used + archive;
    eprintln!(
        "          idle memory: {} MiB ({used} KiB used and the {archive} KiB startup archive)",
        total / 1024
    );
    if total >= IDLE_BUDGET_KIB {
        return Err(format!(
            "the idle installed system uses {total} KiB ({used} KiB, and the {archive} KiB \
             startup archive), not under {IDLE_BUDGET_KIB} KiB"
        ));
    }

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The size of the boot archive in the kernel's log line `Init: the boot
/// archive is N KiB with M entries`.
fn archive_kib(log: &str) -> Result<u64, String> {
    log.lines()
        .find_map(|line| line.split("Init: the boot archive is ").nth(1))
        .and_then(|rest| rest.split(" KiB").next())
        .and_then(|kib| kib.trim().parse().ok())
        .ok_or_else(|| "the kernel did not say how big the boot archive is".to_string())
}

/// The kernel's time, in seconds, on the first line of `log` that holds
/// `needle`: the `[    3.386471]` it starts with.
pub fn log_seconds(log: &str, needle: &str) -> Result<f64, String> {
    log.lines()
        .find(|line| line.contains(needle))
        .and_then(|line| line.split_once('[')?.1.split_once(']'))
        .and_then(|(time, _)| time.trim().parse().ok())
        .ok_or_else(|| format!("no time on the log's line with \"{needle}\""))
}

/// The KiB the screen's `idle-used: N` line gives.
fn idle_used_kib(screen: &[String]) -> Result<u64, String> {
    screen
        .iter()
        .find_map(|line| line.trim().strip_prefix("idle-used: "))
        .and_then(|kib| kib.trim().parse().ok())
        .ok_or_else(|| format!("no idle-used line on the screen: {}", screen.join(" | ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_time_of_a_log_line() {
        let log = "[    0.000000] Framebuffer: console of 160x50 cells\n\
                   [    3.386471] cpu1: spark[1]: consoled: 6 consoles; tty1 is shown\n\
                   [   20.000000] consoled: 6 consoles; tty1 is shown\n";
        let shown = log_seconds(log, TTY1_SHOWN).unwrap();
        assert!((shown - 3.386_471).abs() < 1e-9, "{shown}");
        assert!(log_seconds(log, "no such line").is_err());
        assert!(log_seconds("consoled: 6 consoles; tty1 is shown\n", TTY1_SHOWN).is_err());
    }
}
