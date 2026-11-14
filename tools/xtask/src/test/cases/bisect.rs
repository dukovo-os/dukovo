// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! *changes bisect* finds the change that broke a start (ADR 0475), on an
//! installed system's disk under UEFI. Four changes come between a good
//! start and a failed one: an update, proved in slot B and made the
//! default again by *sysupdate rollback* (slot A held the good start); the
//! boot option *bisect.test.fail=1*, which the update's slot starts with,
//! and which nothing reads until a check does; and two apps installed for
//! everyone, *bisectbad* and *bisectgood*, the first by ID the one whose
//! service ends at once once its data folder holds a file *poison*, and
//! which health.toml names an essential service.
//!
//! The first boot makes the plants and the failure, and the first search:
//! start 4 fails because *bisectbad* does. *changes bisect --yes* restarts
//! the machine by itself, tries slot A with the options of the good start
//! (the bad app still fails it, as it did start 4), then slot B with
//! *bisectbad* left out (which starts well), and names the app, confirmed
//! after two tries. The slot state is as it was, the tries show in
//! *changes --list*, and the result is kept for the next sign-in. The
//! test restarts the machine itself when the health check says it will
//! restart in 60 seconds, as the health check would after them. Then
//! *poison* is taken away and a required check of *bisect.test.fail=1* in
//! the command line is put into health.toml, so that the same image now
//! fails at the option.
//!
//! The second boot is the failed start of the option, and a second
//! search. The power fails at the first log line of its first try: the
//! boot menu had taken the try, so the next start is the usual one (slot
//! B, the default) and the health check tries it again. The tries then
//! name the boot option by elimination after two more, and *changes
//! bisect --confirm* confirms it with one that leaves out only the option.
//! After each search the slot state is unchanged, and *changes --list*
//! marks every try.
//!
//! The restarts are the machine's own starts, so the test takes about 230
//! seconds in the first boot and 190 in the second under KVM, each within
//! the 300 seconds a boot may take.

use super::super::*;
use super::update_stage::{UPDATES, power_off, restart, run, started, type_command};

pub const CASE: Case = Case {
    name: "bisect",
    about: "changes bisect finds the change that broke a start: an update, a boot option and two apps came between a good start and a failed one; the search restarts the machine by itself, tries each half, and names the bad app confirmed after two tries, then, with the bad change moved to the boot option and a power failure during its first try, the option by elimination, which --confirm confirms; the slot state is as it was after each, and the tries show in changes --list (ADR 0475)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: bad_app,
    disks: DISKS,
    second: Some(SecondBoot { cmdline: AS_IT_WAS, before: nothing_between, script: bad_option }),
};

/// The test updates, which Spark mounts at [`UPDATES`], and the two test
/// bundles' files, at [`SERVICES`].
const DISKS: &[Disk] =
    &[update_releases_disk(64 * 1024, "UPDATES"), service_files_disk(8 * 1024, "SERVICES")];

/// Where Spark mounts the bundles' disk.
const SERVICES: &str = "/volumes/vdb";

/// The bundle files, as the test's bundle disk holds them.
const BAD_FILE: &str = "org.example.bisectbad-1.0.0.dkv";
const GOOD_FILE: &str = "org.example.bisectgood-1.0.0.dkv";

/// The file whose presence ends the bad app's service at once: in its
/// data folder, which only root can write.
const POISON: &str = "/shared/services/org.example.bisectbad/bisectbad/poison";

/// health.toml(5) for the first search: the bad app's service is among
/// the essential ones (it is no service until the app is there), and an
/// update proves itself after 10 seconds.
const HEALTH: &str = "printf 'essential = [\"logd\", \"authd\", \"consoled\", \"netd\", \"sshd\", \
                      \"bisectbad\"]\\nstable = 10\\n' > /system/settings/sysupdate/health.toml";

/// health.toml for the second: the same, and a required check that runs
/// the program [`CHECK_PROGRAM`] writes.
const HEALTH_CHECKED: &str = "printf 'essential = [\"logd\", \"authd\", \"consoled\", \"netd\", \
                              \"sshd\", \"bisectbad\"]\\nstable = 10\\n\\n[[check]]\\nname = \
                              \"bisect option\"\\ncommand = [\"/system/commands/sh\", \
                              \"/system/settings/bisect-check\"]\\nrequired = true\\n' > \
                              /system/settings/sysupdate/health.toml";

/// The check's program, which the second search's failure is: it ends with
/// 1 while the boot option the update's slot starts with is on the
/// command line.
const CHECK_PROGRAM: &str = "printf '#!/bin/sh\\nif /system/commands/grep -q bisect.test.fail=1 \
                             /kernel/cmdline; then\\n  echo \"bisect.test.fail=1 is set\"\\n  \
                             exit 1\\nfi\\n' > /system/settings/bisect-check";

/// How many seconds a notification gets to show before a key press takes
/// it off.
const NOTICE_SECONDS: u64 = 1;

/// The first boot: the plants, start 4's failure, and the search that
/// names the bad app.
fn bad_app(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let version = test_update_version("bisect");
    phase("the good start", good_start(vm, &version))?;
    phase("the failed start", failed_start(vm, &version))?;
    phase("the first search", first_search(vm, &version))?;
    // Between the searches: the bad app runs again, and the second
    // search's failure is a check of the boot option.
    run(vm, &format!("rm {POISON} && echo \"fine: $((6 * 8))\""), &["fine: 48"])?;
    run(vm, &format!("{CHECK_PROGRAM} && echo \"program: $((5 * 9))\""), &["program: 45"])?;
    run(vm, &format!("{HEALTH_CHECKED} && echo \"checked: $((7 * 7))\""), &["checked: 49"])?;
    // QEMU writes the boot image to a snapshot it drops when it ends: what
    // is synced now is written to the image, for the second boot.
    run(vm, "sync && echo \"synced: $((4 * 4))\"", &["synced: 16"])?;
    vm.monitor("commit boot")?;
    power_off(vm)
}

/// Starts 1 to 3: slot A starts, an update is staged, tried and proved in
/// slot B, and slot A is made the default again and starts: the good
/// start, 3.
fn good_start(vm: &mut Vm, version: &str) -> Result<(), String> {
    started(vm, 'a')?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vda at {UPDATES}"))?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {SERVICES}"))?;
    run(vm, &format!("{HEALTH} && echo \"short: $((6 * 7))\""), &["short: 42"])?;
    run(
        vm,
        &format!("sysupdate stage {UPDATES}/bisect"),
        &[&format!("sysupdate: staged Dukovo {version} in slot B: it starts at the next restart")],
    )?;
    restart(vm)?;
    started(vm, 'b')?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {version}) is an update being tried: it proves itself when \
         its essential services run for 10 seconds"
    ))?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {version}) proved itself: it is now the default, and slot A \
         (Dukovo {VERSION}) the previous version"
    ))?;
    run(
        vm,
        "sysupdate rollback --keep-settings",
        &[&format!(
            "sysupdate: slot A (Dukovo {VERSION}) starts from the next restart on; restart to go \
             back to it"
        )],
    )?;
    restart(vm)?;
    started(vm, 'a')?;
    // The record of this start is whole once the check says so.
    vm.expect(&format!("sysupdate: this start of slot A (Dukovo {VERSION}) is seen through"))
}

/// Start 4: the apps are installed for everyone, the bad one is poisoned,
/// and slot B is the default again; the start fails at the bad app's
/// service, and offers the search.
fn failed_start(vm: &mut Vm, version: &str) -> Result<(), String> {
    run(
        vm,
        &format!(
            "cd {SERVICES}; elevate bundle install --for-everyone --yes {BAD_FILE} | grep \
             installed; elevate bundle install --for-everyone --yes {GOOD_FILE} | grep \
             installed; cd; echo \"apps: $((6 * 7))\""
        ),
        &[
            "installed org.example.bisectbad 1.0.0, for everyone",
            "installed org.example.bisectgood 1.0.0, for everyone",
            "apps: 42",
        ],
    )?;
    run(
        vm,
        &format!(
            "echo poison > {POISON} && sysupdate rollback --keep-settings && echo \"ready: $((6 * \
             7))\""
        ),
        &[
            &format!(
                "sysupdate: slot B (Dukovo {version}) starts from the next restart on; restart \
                 to go back to it"
            ),
            "ready: 42",
        ],
    )?;
    restart(vm)?;
    started(vm, 'b')?;
    vm.expect(&format!("sysupdate: slot B (Dukovo {version}) failed its health check: "))?;
    vm.expect("(BOOT-102)")?;
    // The failed start says so on the screen's last line until a key is
    // pressed (ADR 0501), and what it kept says how to look into it.
    vm.expect_screen(&["Start 4 failed its health check: "])?;
    vm.press(&["shift"])?;
    vm.expect_screen_without("changes --failed says what changed")?;
    run(
        vm,
        "cat /system/live/sysupdate/failure",
        &["Start 4 failed its health check: ", "elevate changes bisect", "up to 2 restarts."],
    )
}

/// The search of start 4: its plan, then two tries, each a start of its
/// own, and the app named confirmed.
fn first_search(vm: &mut Vm, version: &str) -> Result<(), String> {
    // What the search would do, looked at and not taken: a search
    // declined ends with status 1, as nothing was done.
    type_command(vm, "changes bisect; echo \"status $?\"")?;
    answer_prompt(vm, "[y/N]", "n")?;
    answer_prompt_within(vm, "#", "", 60)?;
    vm.expect_screen(&[
        "Start 4 failed its health check: ",
        "From the last good start, 3, these changes can each be left out for one start:",
        &format!("the system: Dukovo {VERSION} to Dukovo {version}"),
        "the boot options: ",
        "bisect.test.fail=1",
        "app org.example.bisectbad: new, 1.0.0",
        "app org.example.bisectgood: new, 1.0.0",
        "Finding which one takes up to 2 restarts.",
        "changes bisect: nothing was written",
        "status 1",
    ])?;

    // Try 1 leaves out the slot and the options, so the bad app still
    // fails it, as it did start 4. The machine restarts by itself.
    type_command(vm, "changes bisect --yes")?;
    vm.expect("spark: asked to restart the machine")?;
    tried(vm, 'a', 1, "nothing")?;
    vm.expect(&format!("sysupdate: slot A (Dukovo {VERSION}) failed its health check: "))?;
    vm.expect("(BOOT-102)")?;
    vm.expect("changes bisect: restarting in 60 seconds for try 2")?;
    dismiss(vm)?;
    restart(vm)?;

    // Try 2 leaves out the bad app alone, and starts well.
    tried(vm, 'b', 2, "bisectbad")?;
    vm.expect(&format!("sysupdate: this start of slot B (Dukovo {version}) is seen through"))?;
    vm.expect("changes bisect: The start fails with org.example.bisectbad 1.0.0")?;
    dismiss(vm)?;
    run(
        vm,
        "changes bisect --status",
        &[
            "Try 1, start 5, left out 1, 2: it failed as start 4 did",
            "Try 2, start 6, left out 3: it started well",
            "The start fails with org.example.bisectbad 1.0.0",
            ", and starts without it.",
            "Nothing was changed: this start was the last try",
        ],
    )?;
    // The result waits for whoever signs in next (ADR 0504).
    run(
        vm,
        "cat /system/live/sign-in/bisect",
        &["changes bisect: The start fails with org.example.bisectbad 1.0.0"],
    )?;
    slot_state_unchanged(vm, version)?;
    run(
        vm,
        "changes --list",
        &[
            ", slot A, try 1 of changes bisect: failed: ",
            ", slot B, try 2 of changes bisect: good",
        ],
    )
}

/// The second boot: the failed start of the boot option, the search whose
/// first try the power failure cuts, the option named by elimination, and
/// confirmed.
fn bad_option(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let version = test_update_version("bisect");
    phase("the failed start", failed_at_the_option(vm, &version))?;
    phase("the second search", second_search(vm, &version))?;
    phase("the confirming try", confirm(vm, &version))?;
    power_off(vm)
}

/// Start 7: slot B starts again, the bad app runs, and the check of the
/// boot option fails it.
fn failed_at_the_option(vm: &mut Vm, version: &str) -> Result<(), String> {
    started(vm, 'b')?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {version}) failed its health check: the required check bisect \
         option failed: "
    ))?;
    vm.expect("(BOOT-103)")?;
    vm.expect_screen(&[
        "Start 7 failed its health check: the required check bisect option failed",
    ])?;
    vm.press(&["shift"])?;
    vm.expect_screen_without("changes --failed says what changed")?;
    run(
        vm,
        "cat /system/live/sysupdate/failure",
        &["Start 7 failed its health check: ", "up to 2 restarts."],
    )
}

/// The search of start 7. Try 1 is cut by a power failure at its first
/// log line, after the boot menu took it: the start after is the usual
/// one, slot B, and the health check tries it again, as try 2, which
/// starts well; try 3 leaves out the slot alone and fails as start 7 did.
fn second_search(vm: &mut Vm, version: &str) -> Result<(), String> {
    type_command(vm, "changes bisect --yes")?;
    vm.expect("spark: asked to restart the machine")?;
    vm.expect("spark: started slot a, @system.a")?;
    vm.expect("spark: try 1 of changes bisect: leaving out nothing")?;
    vm.monitor("system_reset")?;

    // The usual start: the try was taken, so it is not started again, and
    // it says nothing of a try.
    started(vm, 'b')?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {version}) failed its health check: the required check bisect \
         option failed: "
    ))?;
    vm.expect("changes bisect: restarting in 60 seconds for try 2")?;
    // The kernel quotes the cut start's last lines from its crash log,
    // each after a `| `: those are no start of their own.
    let log = vm.log();
    let tries = log
        .lines()
        .filter(|line| line.contains("of changes bisect: leaving out") && !line.contains("| ["))
        .count();
    if tries != 1 {
        return Err(format!(
            "{tries} starts were tries that leave services out, not the one the power cut"
        ));
    }
    dismiss(vm)?;
    restart(vm)?;

    // Try 2, the same again: it leaves out the slot and the options, and
    // starts well.
    tried(vm, 'a', 2, "nothing")?;
    vm.expect(&format!("sysupdate: this start of slot A (Dukovo {VERSION}) is seen through"))?;
    vm.expect("changes bisect: restarting in 60 seconds for try 3")?;
    dismiss(vm)?;
    restart(vm)?;

    // Try 3 leaves out the slot alone: the option is still on the command
    // line, and fails the start as it did start 7. Nothing else is left.
    tried(vm, 'a', 3, "nothing")?;
    vm.expect(&format!(
        "sysupdate: slot A (Dukovo {VERSION}) failed its health check: the required check bisect \
         option failed: "
    ))?;
    vm.expect("changes bisect: The start fails with the boot options ")?;
    dismiss(vm)?;
    run(
        vm,
        "changes bisect --status",
        &[
            "Try 1, left out 1, 2: it never reached its health check",
            "Try 2, start 9, left out 1, 2: it started well",
            "Try 3, start 10, left out 1: it failed as start 7 did",
            // A screen line is 160 characters: the sentence goes on past it.
            "named by elimination",
        ],
    )?;
    slot_state_unchanged(vm, version)?;
    run(
        vm,
        "changes --list",
        &[
            ", slot A, try 2 of changes bisect: good",
            ", slot A, try 3 of changes bisect: failed: ",
        ],
    )
}

/// *--confirm*: one more try, leaving out only the option, which starts
/// well and confirms it.
fn confirm(vm: &mut Vm, version: &str) -> Result<(), String> {
    type_command(vm, "changes bisect --confirm --yes")?;
    vm.expect("spark: asked to restart the machine")?;
    tried(vm, 'b', 4, "nothing")?;
    vm.expect(&format!("sysupdate: this start of slot B (Dukovo {version}) is seen through"))?;
    vm.expect("changes bisect: The start fails with the boot options ")?;
    dismiss(vm)?;
    run(
        vm,
        "changes bisect --status",
        &[
            "Try 4, start 11, to confirm, left out 2: it started well",
            ", and starts without it.",
        ],
    )?;
    slot_state_unchanged(vm, version)?;
    run(vm, "changes --list", &[", slot B, try 4 of changes bisect: good"])
}

/// Waits for a start that is try `number` of changes bisect, in `slot`,
/// to show tty1, and for what Spark says it leaves out.
fn tried(vm: &mut Vm, slot: char, number: u8, left_out: &str) -> Result<(), String> {
    vm.expect(&format!("spark: started slot {slot}, @system.{slot}"))?;
    vm.expect(&format!("spark: try {number} of changes bisect: leaving out {left_out}"))?;
    vm.expect("; tty1 is shown")?;
    Ok(())
}

/// The slot state, which no search changes: the update is the default,
/// slot A the previous version, and slot B starts next.
fn slot_state_unchanged(vm: &mut Vm, version: &str) -> Result<(), String> {
    run(
        vm,
        "sysupdate status",
        &[
            &format!("Slot A:   Dukovo {VERSION}, the previous version"),
            &format!("Slot B:   Dukovo {version}, the default"),
            "Next start: slot B",
        ],
    )
}

/// Takes off the screen what was sent as a notification a moment ago, as
/// a key press does (ADR 0501), so that the prompt is the screen's last
/// line again: it waits a second for the notification to show first.
fn dismiss(vm: &mut Vm) -> Result<(), String> {
    std::thread::sleep(std::time::Duration::from_secs(NOTICE_SECONDS));
    vm.press(&["shift"])
}

/// `result`, with the part of the test it is of.
fn phase<T>(part: &str, result: Result<T, String>) -> Result<T, String> {
    result.map_err(|err| format!("{part}: {err}"))
}
