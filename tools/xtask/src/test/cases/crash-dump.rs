// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A kernel panic leaves a crash dump that the next start saves (ADR
//! 0483). An installed system's disk boots under UEFI with the self-test
//! *crash-reboot*, which panics at the first start; the restart keeps the
//! RAM, so the second start finds the dump and says it is kept for
//! dumps(8), and the *dumps* service saves it in /system/state/crash and
//! names the panic in the log, as a warning. The dump is whole, not cut
//! short. alice, the owner, signs in. She is not root, so the device and
//! *dumps* refuse her, and *dumps test* does not panic the kernel for
//! her. As an administrator, through elevate, she finds the one dump
//! listed with the self-test's panic, and reads its sections (the panic,
//! the CPUs, the threads, the run queues, the locks, the memory, a stack,
//! and the crashed start's log) and the self-test's function in the
//! backtrace or the stack. The saved file and its folder are root's
//! alone, a second *dumps save* finds the dump saved already, and *dumps
//! test* with no terminal to ask at does not panic the kernel. With
//! *--yes* it does: the kernel says which process asked and panics, and
//! the third start keeps that dump and saves it too. The power button
//! ends it.

use super::super::*;

pub const CASE: Case = Case {
    name: "crash-dump",
    about: "a kernel panic leaves a crash dump that the next start saves (ADR 0483): the second start keeps the dump for dumps(8), the dumps service saves it whole to /system/state/crash and warns in the log; a user who is no administrator cannot read it, an administrator lists and reads it through elevate, with its sections, the self-test's function in the backtrace and the crashed start's log, in files only root reaches; a second save finds it saved, dumps test without a terminal does not panic, and dumps test --yes panics the kernel, whose next start saves that dump too",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    // The self-test panics at the first start and passes at the second;
    // Spark starts after it, with the sign-in on as at any start. A panic
    // restarts at once.
    cmdline: "selftest=crash-reboot init-after-selftest panic=0",
    cpus: 2,
    // With more than 4 GiB the crash log goes above 4 GiB, where the
    // firmware leaves the RAM alone, as in crash-log-uefi.
    memory: "5G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: crash_dump,
    disks: &[],
    second: None,
};

/// alice's password.
const PASSWORD: &str = "queen of hearts";

/// Where the dumps are saved.
const FOLDER: &str = "/system/state/crash";

/// alertd's notification of the panic (ADR 0485), which consoled shows on
/// the bottom line. It is critical, so it stays there until a key is
/// pressed. notifyd sends a notification only to the programs following
/// it then, so whether it shows depends on whether consoled follows
/// notifyd by the time alertd sends it.
const PANIC_NOTICE: &str = "alertd (alerts): The machine restarted after a kernel panic";

/// [`answer_prompt`], after taking the panic's notification off the
/// bottom line whenever it is there: below the prompt, it would be the
/// screen's last line. Shift takes it off and types nothing.
fn answer(vm: &mut Vm, prompt: &str, text: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if vm.screen()?.iter().any(|line| line.contains(PANIC_NOTICE)) {
            vm.press(&["shift"])?;
        }
        match answer_prompt_within(vm, prompt, text, 2) {
            Err(_) if Instant::now() < deadline => {}
            result => return result,
        }
    }
}

/// `command` as root: alice, the owner, is an administrator, and elevate
/// reads her password from its standard input with `-S`.
fn as_root(command: &str) -> String {
    format!("echo '{PASSWORD}' | elevate -S {command}")
}

fn crash_dump(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    // The first start: the self-test panics, and the machine restarts.
    vm.expect("Crash log: boot 1;")?;
    vm.expect("selftest crash-reboot: panicking now")?;
    vm.expect("Radix panic on CPU 0")?;
    vm.expect("selftest crash-reboot: deliberate panic")?;

    // The second start finds the dump, whole, and keeps it for dumps(8).
    vm.expect("Crash log: boot 2; the previous boot (1) ended with a kernel panic")?;
    let kept = program_line(vm, "Crash log: the previous boot's dump is kept for dumps(8) (")?;
    if kept.contains("cut short") {
        return Err(format!("the kernel cut the dump of a small panic short: {kept}"));
    }
    vm.expect("selftest crash-reboot: passed")?;
    vm.expect("Boot complete.")?;

    // The service saves it and says so, as a warning, with the panic's
    // message and where the dump is.
    vm.expect_anywhere("spark: started dumps (/system/commands/dumps) as process ")?;
    vm.expect_anywhere(&format!(
        "dumps: the previous boot ended with a kernel panic: selftest crash-reboot: deliberate \
         panic; the dump is {FOLDER}/"
    ))?;

    // The machine has no account yet: the owner's comes first, as in
    // the signin test.
    vm.expect_anywhere("authd: 0 accounts; answering at /system/live/authd/socket")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    vm.expect_screen(&[
        &format!("{RELEASE} {VERSION}"),
        "This computer has no account yet: make yours, the owner's,",
    ])?;
    answer(vm, "Name:", "alice")?;
    answer(vm, "Full name (may be empty):", "Alice Liddell")?;
    answer(vm, "New password:", PASSWORD)?;
    answer(vm, "The same again:", PASSWORD)?;
    vm.expect_anywhere("authd: alice signed in on pts/")?;
    vm.expect_screen(&["Made alice, number 1000, home /users/alice"])?;

    // She is no root: the device is root's, and dumps says to elevate.
    // Nothing she types panics the kernel. Each line ends with a command
    // that succeeds: after one that fails, the shell's hint follows the
    // prompt.
    answer(vm, "$", "clear; cat /devices/crashdump; echo \"cat $?\"")?;
    vm.expect_screen(&["Permission denied", "cat 1"])?;
    answer(
        vm,
        "$",
        "clear; dumps; echo \"list $?\"; dumps show; echo \"show $?\"; dumps test --yes; echo \
         \"test $?\"",
    )?;
    vm.expect_screen(&[
        "dumps: only root reads the crash dumps; try elevate dumps",
        "list 1",
        "show 1",
        "test 1",
    ])?;

    // Through elevate she lists the one dump: its name ends with the boot's
    // number, and its panic is the self-test's. `list` is read through a
    // pipe, so that no terminal's width cuts the panic short.
    answer(
        vm,
        "$",
        &format!(
            "clear; {} > /tmp/l.txt; cat /tmp/l.txt; echo \"rows $(grep -c 'boot1[.]txt' \
             /tmp/l.txt)\"; {}",
            as_root("dumps list"),
            as_root("dumps path"),
        ),
    )?;
    vm.expect_screen(&[
        "NAME",
        "BOOT",
        "SIZE",
        "PANIC",
        "-boot1.txt",
        "selftest crash-reboot: deliberate panic",
        "rows 1",
        &format!("{FOLDER}/"),
    ])?;

    // What the dump holds: its three header lines, and its sections in
    // order. The self-test's function is in the panic's backtrace or on the
    // stack; the panic message names it with a hyphen, so it is not that.
    answer(
        vm,
        "$",
        &format!(
            "clear; {} > /tmp/d.txt; head -n 3 /tmp/d.txt; grep -e '^== ' /tmp/d.txt",
            as_root("dumps show")
        ),
    )?;
    vm.expect_screen(&[
        "Radix crash dump",
        &format!("Radix {VERSION} on x86_64, boot 1"),
        "Panic at ",
        "== Panic",
        "== CPUs",
        "== Threads",
        "== Run queues",
        "== Locks",
        "== Memory",
        "== Stack of CPU 0",
        "== Log",
    ])?;
    // The self-test's function is in the backtrace or on the stack, and the
    // crashed start's log follows `== Log`, with the line the self-test
    // logged before it panicked. The dump was not cut short, and the kernel
    // finished it.
    answer(
        vm,
        "$",
        "clear; awk '/^== Log$/ { exit } /crash_reboo[t]/ { found = 1 } END { if (found) print \
         \"frame: \" 40 + 2 }' /tmp/d.txt; awk '/^== Log$/ { in_log = 1 } in_log && \
         /selftest crash-reboot: panicking no[w]/ { found = 1 } END { if (found) print \
         \"crashed-log: \" 40 + 2 }' /tmp/d.txt; echo \"cut $(grep -c 'cut short her[e]' \
         /tmp/d.txt) unfinished $(grep -c 'dump ends her[e]' /tmp/d.txt)\"",
    )?;
    vm.expect_screen(&["frame: 42", "crashed-log: 42", "cut 0 unfinished 0"])?;

    // The saved file and its folder are root's alone.
    answer(
        vm,
        "$",
        &format!(
            "clear; {} | cut -c1-10; {} | cut -c1-10",
            as_root(&format!("ls -ld {FOLDER}")),
            as_root(&format!("ls -l {FOLDER}")),
        ),
    )?;
    vm.expect_screen(&["drwx------", "-rw-------"])?;

    // A second save finds the dump saved.
    answer(vm, "$", &format!("clear; {}", as_root("dumps save")))?;
    vm.expect_screen(&["dumps: the previous boot's dump is already saved as "])?;

    // The log has the warning: a record of the dumps service at that
    // level, with the panic's message.
    answer(
        vm,
        "$",
        &format!(
            "clear; {} | grep -c 'the previous boot ende[d] with a kernel panic: selftest \
             crash-reboot: deliberate pani[c]; the dump is {FOLDER}/' | sed 's/^[1-9][0-9]*$/warned/'",
            as_root("log -s dumps -p warning")
        ),
    )?;
    vm.expect_screen(&["warned"])?;

    // `dumps test` asks at a terminal, and with its input a pipe, as here,
    // it will not go on without --yes: the kernel is not asked to panic.
    answer(vm, "$", &format!("clear; {}; echo \"asked $?\"", as_root("dumps test")))?;
    vm.expect_screen(&[
        "this panics the kernel: run it at a terminal to be asked, or give --yes",
        "asked 1",
    ])?;
    answer(vm, "$", "clear; echo \"still $((40 + 2))\"")?;
    vm.expect_screen(&["still 42"])?;

    // With --yes it does: the kernel says which process asked, panics, and
    // restarts. The third start keeps that dump whole too, and the service
    // saves it, naming the test's panic.
    answer(vm, "$", &format!("clear; {}", as_root("dumps test --yes")))?;
    vm.expect("Crash dump: a test panic was asked for by process ")?;
    vm.expect("Radix panic on CPU ")?;
    vm.expect("dumps test: a panic asked for on purpose")?;
    vm.expect("Crash log: boot 3; the previous boot (2) ended with a kernel panic")?;
    let kept = program_line(vm, "Crash log: the previous boot's dump is kept for dumps(8) (")?;
    if kept.contains("cut short") {
        return Err(format!("the kernel cut the test panic's dump short: {kept}"));
    }
    // The second start's log holds the first panic's message, so the
    // self-test finds that the panic before was kept, and goes on.
    vm.expect("selftest crash-reboot: passed")?;
    // The service and the consoles start side by side, in either order.
    vm.expect_ahead(&format!(
        "dumps: the previous boot ended with a kernel panic: dumps test: a panic asked for on \
         purpose; the dump is {FOLDER}/"
    ))?;
    vm.expect_ahead("consoled: started /system/commands/login on tty1 as process ")?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gives_elevate_the_password_on_its_input() {
        assert_eq!(as_root("dumps list"), "echo 'queen of hearts' | elevate -S dumps list");
    }
}
