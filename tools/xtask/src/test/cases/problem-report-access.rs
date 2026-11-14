// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Who may see what of problem reports (ADRs 0531, 0532), and the kernel's
//! records of crashes with nothing reading them (ADR 0530). The machine is
//! set up as in problem-report: an installed system's disk under UEFI, the
//! owner alice, the staging server on the host and the test's two services.
//! The service that crashes does so once, and reportd keeps the crash.
//!
//! A failed start's notice, written by hand, brings its keys to the sign-in
//! screen: Q draws the support page's QR code and address, and R asks for an
//! administrator's name and password and shows the failed start's report,
//! which C cancels. bob, who is no administrator, sees none of the crashes
//! and is refused the service's log, which alice, an administrator, has.
//!
//! With reportd stopped, nine crashes leave the kernel's records of eight: a
//! read of them into a buffer that is too small is refused, and once reportd
//! runs again its log says that one crash came too fast to be kept. The
//! power button ends it.

use super::super::*;
use super::backup::elevated;
use super::problem_report::{
    CHOICES, CRASH_SECONDS, Host, PASSWORD, QUESTIONS, SEND_SECONDS, expect_qr_code,
    owner_signs_in, run, set_up, shown_within, wait_for_end,
};

pub const CASE: Case = Case {
    name: "problem-report-access",
    about: "problem reports (ADRs 0530-0532), who sees what: a failed start's notice brings the QR code and the report keys to the sign-in screen, and R reports the failed start after an administrator signs in; a user who is no administrator sees none of the crashes or the service's log; with reportd stopped the kernel keeps eight records of nine crashes, refuses a read too small for one, and reportd counts the one dropped",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: problem_report_access,
    disks: &[],
    second: None,
};

/// bob's password.
const BOB_PASSWORD: &str = "white rabbit";

/// Where the test keeps its files on the host, from the top of the tree.
const DIR: &str = "out/test/problem-report-access";

/// What the sign-in screen offers when a failed start has left its notice,
/// and the notice the test writes by hand: its first line and the support
/// page of its error.
const KEYS: &str =
    "Press R to report this failure, Q for its support page as a QR code, or Enter to sign in.";
const NOTICE_START: &str = "Start 2 failed its health check";
const SUPPORT: &str = "https://dukovo.org/support/0.15.0/errors/BOOT-101.html";

/// The lines of a script that writes the failed start's notice, which
/// sysupdate(8) writes at a start that fails its check (ADR 0501).
const NOTICE: &[&str] = &[
    "mkdir -p /system/live/sysupdate",
    "echo \"Start 2 failed its health check: the network card is missing (BOOT-101)\" > \
     /system/live/sysupdate/failure",
    "echo \"Dukovo went on with this start.\" >> /system/live/sysupdate/failure",
    "echo \"Help: https://dukovo.org/support/0.15.0/errors/BOOT-101.html\" >> \
     /system/live/sysupdate/failure",
];


fn problem_report_access(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let host = Host::start(DIR)?;
    owner_signs_in(vm)?;
    set_up(vm, &host)?;
    run(vm, "crash crash-test signal 1", CRASH_SECONDS, &["kept signal 1"])?;
    the_failure_notice_has_its_keys(vm)?;
    a_user_sees_only_their_own(vm, &host)?;
    the_kernel_keeps_eight_records(vm)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// A failed start's notice, written as sysupdate(8) writes it, shows the
/// keys when alice signs out. Q shows the QR code of the support page and
/// the address, and any key goes back. R asks for an administrator's name
/// and password, and the session shows the failed start's report; C cancels
/// it, and Enter signs in.
fn the_failure_notice_has_its_keys(vm: &mut Vm) -> Result<(), String> {
    elevated(vm, "notice.sh", NOTICE, false)?;
    answer_prompt(vm, "$", "exit")?;
    wait_for_end(vm, "or Enter to sign in.", SEND_SECONDS)?;
    vm.expect_screen(&[NOTICE_START, KEYS])?;
    vm.type_text("Q")?;
    shown_within(vm, &["Press a key to go back.", SUPPORT], SEND_SECONDS)?;
    expect_qr_code(vm, "the support page's address")?;
    vm.type_text("\n")?;

    vm.type_text("R")?;
    shown_within(vm, &["Sign in as an administrator to report it."], SEND_SECONDS)?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", PASSWORD)?;
    for question in QUESTIONS {
        shown_within(vm, &[question], SEND_SECONDS)?;
        vm.type_text("\n")?;
    }
    // The text is in the pager, which `q` leaves, where there is one; the
    // answers were skipped, so the notice is its first section.
    shown_within(vm, &["== Failed start", NOTICE_START], SEND_SECONDS)?;
    vm.type_text("q")?;
    shown_within(vm, &[CHOICES], SEND_SECONDS)?;
    vm.type_text("C")?;
    wait_for_end(vm, "or Enter to sign in.", SEND_SECONDS)?;
    vm.type_text("\n")?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", PASSWORD)
}

/// alice, an administrator, sees every crash and the service's log. bob,
/// whom she makes, is refused the log, sees none of the crashes (none was
/// his program's), and `report crash` has nothing for him.
fn a_user_sees_only_their_own(vm: &mut Vm, host: &Host) -> Result<(), String> {
    answer_prompt(vm, "$", "clear; elevate user add bob")?;
    answer_prompt(vm, "user add bob:", PASSWORD)?;
    answer_prompt(vm, "New password:", BOB_PASSWORD)?;
    answer_prompt(vm, "The same again:", BOB_PASSWORD)?;
    shown_within(vm, &["Made bob, number 1001, home /users/bob"], SEND_SECONDS)?;
    // The notice of the failed start goes, so that the sign-in screens are
    // as they were; the crash of the service is alice's to see.
    run(
        vm,
        ". ./functions.sh; elevate rm /system/live/sysupdate/failure; echo \"admin sees \
         $(crashes | grep -c signature) draft $(service_draft | grep -c 'has_logs.:true')\"",
        SEND_SECONDS,
        &["admin sees 1 draft 1"],
    )?;

    vm.press(&["alt-f2"])?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty2 as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION}")])?;
    answer_prompt(vm, "Name:", "bob")?;
    answer_prompt(vm, "Password:", BOB_PASSWORD)?;
    vm.expect_anywhere("authd: bob signed in on pts/")?;
    let functions = format!(
        "curl -sf http://10.0.2.2:{}/functions.sh -o functions.sh; . ./functions.sh",
        host.files_port
    );
    run(
        vm,
        &format!(
            "{functions}; echo \"bob sees $(crashes | grep -c signature) draft \
             $(service_draft | grep -c error) logs $(service_draft | grep -c 'has_logs.:true')\""
        ),
        SEND_SECONDS,
        &["bob sees 0 draft 1 logs 0"],
    )?;
    run(
        vm,
        "if report crash < /dev/null > /dev/null 2>&1; then echo \"bob reported\"; else echo \"bob \
         refus\"\"ed\"; fi; echo \"listed $(report list | grep -c crash-test)\"",
        SEND_SECONDS,
        &["bob refused", "listed 0"],
    )
}

/// With reportd stopped nothing reads the kernel's records of crashes: it
/// keeps those of eight, and drops the ninth. A read into a buffer too small
/// for a record is refused, and the record stays. Once reportd runs again it
/// reads what was kept, and the log says how many crashes came too fast to
/// be kept: the first record it reads carries the count. The step is the
/// last, in alice's shell on the first console, as it adds to the crashes
/// that the one before counts.
fn the_kernel_keeps_eight_records(vm: &mut Vm) -> Result<(), String> {
    vm.press(&["alt-f1"])?;
    run(
        vm,
        "elevate service reportd stop; for i in 1 2 3 4 5 6 7 8 9; do \
         /system/tests/report/crash-test segv 2> /dev/null; done; elevate dd \
         if=/devices/crashes of=/dev/null bs=64 count=1 2>&1; echo \"read $((6 * 7))\"; true",
        CRASH_SECONDS,
        &["IO error: Invalid input", "read 42"],
    )?;
    run(
        vm,
        "elevate service reportd start; i=0; while [ $i -lt 60 ] && ! \
         /system/tests/report/crash-test ask '{\"ask\":\"queue\"}' > /dev/null 2>&1; do sleep 1; \
         i=$((i + 1)); done; i=0; while [ $i -lt 60 ] && [ \"$(elevate log -s reportd | grep -c '1 crashes came too fas[t]')\" -lt 1 \
         ]; do sleep 1; i=$((i + 1)); done; echo \"told $(elevate log -s reportd | grep -c '1 \
         crashes came too fas[t]')\"",
        CRASH_SECONDS,
        &["told 1"],
    )
}
