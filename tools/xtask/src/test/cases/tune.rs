// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Performance profiles (ADR 0476), at the console as an administrator.
//! The tune service applies the default at boot and, as QEMU's CPUs show
//! a hypervisor, leaves one notice for the sign-in screens that *elevate
//! tune set vm-guest* suits the machine. *tune* shows the profile in use
//! and each kernel setting's value, including whether the CPUs may boost
//! and how they are held, *tune list* and *tune show* the profiles
//! (benchmark among them, ADR 0557) and what each sets and why, and
//! *tune recommend* says vm-guest. *tune set throughput* sets the slice
//! and the read-ahead in /kernel/settings, and says in one line that the
//! CPUs cannot have an energy preference, which QEMU's lack, while the
//! rest was set; *tune* then marks what the profile set. *tune set
//! balanced* leaves every file of /kernel/settings at the value it
//! started with, and a profile that is no profile is refused. Choosing a
//! profile ends the notice, and vm-guest sets what it lists and nothing
//! else.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "tune",
    about: "performance profiles (ADR 0476): tune shows the default and each setting; list and show say the profiles and why each setting is set; recommend says vm-guest; set throughput sets the slice and the read-ahead and says in one line that QEMU's CPUs cannot have an energy preference; set balanced leaves every file of /kernel/settings at its start value; the notice about virtual machines is made once and ends when a profile is chosen",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: tune,
    disks: &[],
    second: None,
};

/// What each of the kernel's settings holds, a line each, as the files
/// everyone reads show it.
const SETTINGS: &str = "for f in /kernel/settings/*; do echo \"$f $(cat $f)\"; done";

fn tune(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    // The service ran at boot, with no profile chosen: nothing to set.
    vm.expect_anywhere("spark: started tune (/system/commands/tune) as process ")?;
    vm.expect_anywhere("tune: profile balanced: nothing to change")?;

    // The notice for the sign-in screens, made once, and marked as given
    // so that the next boot leaves it out.
    answer_prompt(
        vm,
        "#",
        "clear; cat /system/live/sign-in/tune | tr ':' '\\n'; [ -e /system/state/tune/vm-notice ] \
         && echo \"notice marked\"",
    )?;
    vm.expect_screen(&["This machine runs in a virtual machine", "suits it.", "notice marked"])?;

    // The default, and what each setting holds, none marked.
    answer_prompt(vm, "#", &format!("clear; tune; {SETTINGS} > /tmp/start"))?;
    vm.expect_screen(&[
        "Profile: balanced, the default; The default: every setting at its start value",
        "cpu.boost              on",
        "cpu.energy_preference  firmware",
        "cpu.speed              chosen",
        "disk.read_ahead_kib    128",
        "net.congestion         cubic",
        "sched.slice_us         3000",
        "* set by the profile",
    ])?;
    answer_prompt(vm, "#", "clear; tune list")?;
    vm.expect_screen(&[
        "balanced     The default: every setting at its start value  (in use)",
        "benchmark    Steady timings: CPUs at their base frequency",
        "low-latency  Quick answers",
        "power-saver  Less power",
        "throughput   Most work done a second",
        "vm-guest     A virtual machine",
        "vm-host      A machine that runs virtual machines",
    ])?;
    answer_prompt(vm, "#", "clear; tune show benchmark")?;
    vm.expect_screen(&[
        "benchmark: Steady timings: CPUs at their base frequency",
        "cpu.boost              off          Turbo bins depend on heat",
        "cpu.energy_preference  performance  The CPUs ask for the most speed",
        "cpu.speed              highest      Each CPU is held at its highest",
        "Every other setting is at its start value.",
    ])?;
    answer_prompt(vm, "#", "clear; tune show throughput")?;
    vm.expect_screen(&[
        "throughput: Most work done a second",
        "cpu.energy_preference  performance  The CPUs lean to speed",
        "disk.read_ahead_kib    1024         Reads further ahead",
        "sched.slice_us         6000         Fewer thread switches",
        "Every other setting is at its start value.",
    ])?;
    answer_prompt(vm, "#", "clear; tune recommend")?;
    vm.expect_screen(&["vm-guest: this machine runs in a virtual machine", "chooses it."])?;

    // throughput: the slice and the read-ahead are set; the energy
    // preference, which these CPUs cannot have, is said in one line.
    answer_prompt(vm, "#", "clear; tune set throughput; echo \"set $?\"")?;
    vm.expect_screen(&[
        "the profile is throughput",
        "cpu.energy_preference: this machine cannot have performance; it stays firmware",
        "set 0",
    ])?;
    vm.expect_anywhere(
        "tune: profile throughput: set sched.slice_us to 6000, disk.read_ahead_kib to 1024",
    )?;
    vm.expect_anywhere("tune: cpu.energy_preference: this machine cannot have performance")?;
    answer_prompt(vm, "#", "clear; tune")?;
    vm.expect_screen(&[
        "Profile: throughput, chosen by an administrator",
        "the profile asks for performance; not in effect",
        "* disk.read_ahead_kib    1024",
        "* sched.slice_us         6000",
    ])?;
    // Choosing a profile ends the notice.
    answer_prompt(
        vm,
        "#",
        "clear; [ -e /system/live/sign-in/tune ] || echo \"notice gone\"; cat \
         /kernel/settings/sched.slice_us /kernel/settings/disk.read_ahead_kib | tr '\\n' ' '",
    )?;
    vm.expect_screen(&["notice gone", "6000 1024"])?;

    // balanced puts back everything throughput set, and nothing else is
    // touched: every file is as it started.
    answer_prompt(
        vm,
        "#",
        &format!(
            "clear; tune set balanced; {SETTINGS} > /tmp/now; [ \"$(cat /tmp/now)\" = \"$(cat \
             /tmp/start)\" ] && echo \"settings are at their start\""
        ),
    )?;
    vm.expect_screen(&["the profile is balanced", "settings are at their start"])?;
    vm.expect_anywhere(
        "tune: profile balanced: set sched.slice_us to 3000, disk.read_ahead_kib to 128",
    )?;

    // vm-guest sets what it lists and the energy preference stays at its
    // start; the notice does not come back when the service runs again.
    answer_prompt(
        vm,
        "#",
        "clear; tune set vm-guest; service tune restart -q; cat /kernel/settings/sched.slice_us \
         /kernel/settings/disk.read_ahead_kib /kernel/settings/cpu.energy_preference | tr '\\n' \
         ' '; [ -e /system/live/sign-in/tune ] || echo \"notice gone\"",
    )?;
    vm.expect_screen(&["the profile is vm-guest", "4000 512 firmware", "notice gone"])?;

    // A name that is no profile is refused, and the choice stays.
    answer_prompt(vm, "#", "clear; tune set nonsense; echo \"status $?\"; tune | head -1")?;
    vm.expect_screen(&[
        "tune: no profile is called nonsense; `tune list` names them",
        "status 1",
        "Profile: vm-guest, chosen by an administrator",
    ])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
