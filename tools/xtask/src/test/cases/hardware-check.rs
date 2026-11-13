// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! *hardware check* looks for failing hardware on an installed system's
//! disk under UEFI, with a second SATA disk on a second AHCI controller
//! whose serial number is `hwcheck-disk` and whose reads QEMU's blkdebug
//! fails at sectors 20000, 40000 and 40001. alice, the owner, signs in.
//! She is no administrator, so *hardware check* refuses her with one line
//! and status 3. As an administrator, through elevate, a check of the
//! processors, the sensors, the battery and the network cards that has
//! nothing to find ends with status 0. Then QEMU's *mce* puts a corrected
//! memory error in a machine-check bank, which the kernel logs at its
//! next read, and a quick check of everything on the test disk ends with
//! status 1: the memory test and the load pass, the memory errors and the
//! machine checks warn (HW-101 and HW-121), the disk says it is healthy
//! and its short self-test passes, and its read scan fails and names the
//! sectors, as a range for the two side by side (HW-113). *hardware last*
//! shows the same result and the log has the run's line with its IDs. A
//! check with *--json* prints the file *hardware last --json* reads,
//! whole, and ends with status 2. The power button ends it.

use super::super::*;

pub const CASE: Case = Case {
    name: "hardware-check",
    about: "hardware check on an installed system: a user who is no administrator is refused; through elevate a check with nothing to find ends with status 0, and with a corrected memory error QEMU's mce put in a bank and a second SATA disk whose reads fail at known sectors, a quick check passes the memory test, the load and the disk's health and self-test, warns of the memory errors and the machine checks, and fails the read scan naming the sectors; hardware last and --json show the same result, which is kept, and the log has the run's line",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "mce-poll=1",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: hardware_check,
    disks: DISKS,
    second: None,
};

/// The disk the check reads: 64 MiB, with a serial number the test finds
/// it by, since the kernel names it after the installed system's disk.
const DISKS: &[Disk] = &[Disk {
    kib: 64 * 1024,
    device: ",serial=hwcheck-disk",
    errors: Some(bad_sectors),
    ..raw_disk(Bus::Sata, raw_unchanged)
}];

/// The sectors whose reads fail, each time they are tried.
const BAD_SECTORS: [u64; 3] = [20000, 40000, 40001];

/// blkdebug's rules for [`DISKS`]: a read that includes one of
/// [`BAD_SECTORS`] fails with EIO.
fn bad_sectors(_: usize) -> String {
    rules(&BAD_SECTORS)
}

fn rules(sectors: &[u64]) -> String {
    let rule =
        |sector: &u64| format!("[inject-error]\nevent = \"read_aio\"\nerrno = \"5\"\nsector = \"{sector}\"\n");
    sectors.iter().map(rule).collect::<Vec<_>>().join("\n")
}

/// alice's password.
const PASSWORD: &str = "queen of hearts";

/// `command` as root: alice, the owner, is an administrator, and elevate
/// reads her password from its standard input with `-S`.
fn as_root(command: &str) -> String {
    format!("echo '{PASSWORD}' | elevate -S {command}")
}

/// A line that says how many lines of `file` match `pattern`, as `label
/// 1` for one: the typed command holds no such text, so the screen shows
/// it only when the command has run.
fn count(label: &str, pattern: &str, file: &str) -> String {
    format!("echo \"{label} $(grep -c -E '{pattern}' {file})\"")
}

fn hardware_check(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("authd: 0 accounts; answering at /system/live/authd/socket")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    vm.expect_screen(&[
        &format!("{RELEASE} {VERSION}"),
        "This computer has no account yet: make yours, the owner's,",
    ])?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Full name (may be empty):", "Alice Liddell")?;
    answer_prompt(vm, "New password:", PASSWORD)?;
    answer_prompt(vm, "The same again:", PASSWORD)?;
    vm.expect_anywhere("authd: alice signed in on pts/")?;
    vm.expect_screen(&["Made alice, number 1000, home /users/alice"])?;

    // She is no administrator: one line, and status 3, before anything
    // runs.
    answer_prompt(vm, "$", "clear; hardware check --time 3; echo \"refused $?\"")?;
    vm.expect_screen(&[
        "hardware check needs an administrator: run elevate hardware check",
        "refused 3",
    ])?;

    // The test disk, by its serial number.
    answer_prompt(
        vm,
        "$",
        "clear; for d in /kernel/block/*; do grep -q hwcheck-disk $d/device/serial 2>/dev/null \
         && T=${d##*/}; done; echo \"test disk $T\"",
    )?;
    vm.expect_screen(&["test disk sd"])?;

    // Nothing to find: the load agrees, nothing is hot, no machine check
    // is in the log. What this machine lacks (sensors, a battery) is
    // skipped, not a failure. The result goes to a file, and the next
    // command reads it once the prompt is back.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {} > /tmp/clean.txt; echo $? > /tmp/clean.status",
            as_root("hardware check cpu sensors battery network --time 3")
        ),
    )?;
    let reading = [
        "echo \"clean $(cat /tmp/clean.status)\"".to_string(),
        count("clean-load", "^cpu +load +passed", "/tmp/clean.txt"),
    ];
    answer_prompt_within(vm, "$", &format!("clear; {}", reading.join("; ")), 120)?;
    vm.expect_screen(&["clean 0", "clean-load 1"])?;

    // A corrected memory error in bank 3, as the machine-check test puts
    // one: the kernel logs it at its next read of the banks, and counts it
    // for channel 1 in the EDAC files.
    vm.monitor("mce 0 3 0x9c00000000000091 0 0x20000000 0x86")?;
    vm.expect(
        "Machine check: CPU 0 bank 3: corrected memory read error on channel 1, address \
         0x20000000 (status 0x9c00000000000091, misc 0x86)",
    )?;

    // The quick check of everything on the test disk. The disk's reads
    // fail where blkdebug says, which the kernel tries five times each,
    // so this takes a while.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {} > /tmp/quick.txt; echo $? > /tmp/quick.status",
            as_root("hardware check --disk $T --time 5")
        ),
    )?;
    let quick = "/tmp/quick.txt";
    let lines = [
        "echo \"status $(cat /tmp/quick.status)\"".to_string(),
        count("memory-test", "^memory +memory test +passed", quick),
        count("memory-errors", "^memory +memory errors +warning +1 corrected error ", quick),
        count("ids", "HW-101: ", quick),
        count("health", "^disks +[a-z0-9]+ health +passed", quick),
        count("self-test", "^disks +[a-z0-9]+ self-test +passed", quick),
        count(
            "scan",
            "^disks +[a-z0-9]+ read scan +FAILED +unreadable sectors: 20000, 40000-40001;",
            quick,
        ),
        count("scan-id", "HW-113: ", quick),
        count("load", "^cpu +load +passed", quick),
        count("machine-checks", "^cpu +machine checks +warning +1 corrected error ", quick),
        count("cpu-id", "HW-121: ", quick),
    ];
    answer_prompt_within(vm, "$", &format!("clear; {}", lines.join("; ")), 400)?;
    vm.expect_screen(&[
        "status 1",
        "memory-test 1",
        "memory-errors 1",
        "ids 1",
        "health 1",
        "self-test 1",
        "scan 1",
        "scan-id 1",
        "load 1",
        "machine-checks 1",
        "cpu-id 1",
    ])?;

    // The run's result is kept: hardware last shows it, with the status
    // it ended with.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {} > /tmp/last.txt; echo \"last $?\"; {}; {}",
            as_root("hardware last"),
            count("heading", "^A quick check ended ", "/tmp/last.txt"),
            count("last-scan", "read scan +FAILED +unreadable sectors: 20000, 40000-40001;", "/tmp/last.txt"),
        ),
    )?;
    vm.expect_screen(&["last 1", "heading 1", "last-scan 1"])?;

    // The log has one line for the run, at the level of its status, with
    // the IDs of what it found.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {} | grep -c 'quick check ended with status 1: .*HW-101, HW-113, HW-121' | \
             sed 's/^1$/logged/'",
            as_root("dmesg")
        ),
    )?;
    vm.expect_screen(&["logged"])?;

    // --json prints what the file holds, whole: hardware last --json
    // reads the same. The memory errors and the machine checks warn, so
    // the status is 2.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {} > /tmp/run.json; echo $? > /tmp/run.status; {} > /tmp/last.json",
            as_root("hardware check memory cpu --time 3 --json"),
            as_root("hardware last --json"),
        ),
    )?;
    let reading = [
        "echo \"json $(cat /tmp/run.status)\"".to_string(),
        count("area", "^ +.area.: .memory.,", "/tmp/run.json"),
        count("status-field", "^  .status.: 2,", "/tmp/run.json"),
        "[ \"$(cat /tmp/run.json)\" = \"$(cat /tmp/last.json)\" ] && echo \"same $((40 + 2))\"".to_string(),
    ];
    answer_prompt_within(vm, "$", &format!("clear; {}", reading.join("; ")), 180)?;
    vm.expect_screen(&["json 2", "area 2", "status-field 1", "same 42"])?;

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
        assert_eq!(
            as_root("hardware last"),
            "echo 'queen of hearts' | elevate -S hardware last"
        );
    }

    #[test]
    fn blkdebug_fails_the_reads_of_each_bad_sector() {
        let text = bad_sectors(0);
        assert_eq!(text.matches("[inject-error]").count(), 3);
        assert_eq!(text.matches("event = \"read_aio\"").count(), 3);
        for sector in BAD_SECTORS {
            assert!(text.contains(&format!("sector = \"{sector}\"")), "{text}");
        }
    }

    #[test]
    fn a_count_says_its_label_and_the_number_of_matching_lines() {
        assert_eq!(
            count("load", "^cpu +load", "/tmp/c"),
            "echo \"load $(grep -c -E '^cpu +load' /tmp/c)\""
        );
    }
}
