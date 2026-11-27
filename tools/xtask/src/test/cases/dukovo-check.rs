// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! *dukovo-check* looks at the security and health of an installed
//! system's computer (ADR 0479). The disk boots under UEFI and alice, the
//! owner, signs in. She is no root, so *dukovo-check* refuses her with
//! one line and status 3, before it looks at anything.
//!
//! A check through elevate on the system as it was installed finds no
//! warning and ends with status 0, in under 5 seconds where the machine
//! has KVM: 17 lines, the firewall service said to have loaded its rules,
//! SSH off, alice the one administrator, no channel for alerts beyond the
//! screens (which is no warning), and the snapshots made. Where the
//! computer has been up too short a while for updated and timed to have
//! done their parts, those checks are skipped, not warned. *--last*, as
//! alice, who is in the group admins, shows what the run showed. Then
//! *service sshd enable* turns SSH on, which does not change the result
//! except that the port is among those other machines reach, and sshd_config
//! with *PasswordAuthentication yes* gives SEC-110, and SEC-111 with it, since
//! no list says who may sign in, with status 2 and the run's line in the log
//! with both IDs. *--json* prints the file *--last --json* reads, whole, which
//! has the version, the time, the release, each result and the counts, and is
//! kept with mode 0640 for the group admins. Turned off again, the passwords
//! pass and the status is 0. The power button ends it.

use super::super::*;

pub const CASE: Case = Case {
    name: "dukovo-check",
    about: "dukovo-check on an installed system: a user who is no root is refused with status 3; through elevate the system as installed ends with status 0, 17 lines and no warning, in under 5 seconds with KVM, the alerts line passing with no channel set up; --last as alice, who is in admins, shows the same; with sshd on, PasswordAuthentication yes gives SEC-110 and SEC-111 and status 2, with the run's line and both IDs in the log, and --json and --last --json print the same file, kept with mode 0640 for admins; with passwords off again the check passes",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: dukovo_check,
    disks: &[],
    second: None,
};

/// alice's password.
const PASSWORD: &str = "queen of hearts";

/// Where the kept result is, and sshd's settings.
const RESULT: &str = "/system/state/dukovo-check/last.json";
const SSHD_CONFIG: &str = "/system/settings/ssh/sshd_config";

/// The most seconds a check may take, where the machine has KVM.
const BUDGET_S: u64 = 5;

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

/// A command that runs `dukovo-check` with `options` as root, and keeps
/// what it printed in `/tmp/NAME.txt`, its status in `/tmp/NAME.status`
/// and the seconds it took, by the clock, in `/tmp/NAME.took`.
fn run_check(name: &str, options: &str) -> String {
    let command = format!("dukovo-check {options}");
    format!(
        "s=$(date +%s); {} > /tmp/{name}.txt; echo $? > /tmp/{name}.status; e=$(date +%s); \
         echo $((e - s)) > /tmp/{name}.took",
        as_root(command.trim_end())
    )
}

/// The lines that say how a run ended: its label and status, and the
/// seconds it took.
fn ended(name: &str) -> [String; 2] {
    [
        format!("echo \"{name} $(cat /tmp/{name}.status)\""),
        format!("echo \"{name}-took $(cat /tmp/{name}.took)\""),
    ]
}

/// The number of seconds the screen's `LABEL-took N` line gives.
fn took_seconds(screen: &[String], name: &str) -> Result<u64, String> {
    let prefix = format!("{name}-took ");
    screen
        .iter()
        .find_map(|line| line.trim().strip_prefix(prefix.as_str()))
        .and_then(|seconds| seconds.trim().parse().ok())
        .ok_or_else(|| format!("no {prefix}line on the screen: {}", screen.join(" | ")))
}

/// Types `command` after the prompt of a shell, and waits for it.
fn type_and_wait(vm: &mut Vm, command: &str) -> Result<(), String> {
    answer_prompt_within(vm, "$", &format!("clear; {command}"), 180)
}

/// The patterns of a run's lines, in the order they are counted.
const LINE: &str = "^(network|ssh|accounts|updates|time|services|health|alerts|backups) ";
const WARNED: &str = "^[a-z]+ +[a-z ]+ (warning|FAILED) ";

fn dukovo_check(vm: &mut Vm, _case: &Case) -> Result<(), String> {
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

    // She is no root: one line, and status 3, before anything is looked at.
    answer_prompt(vm, "$", "clear; dukovo-check; echo \"refused $?\"")?;
    vm.expect_screen(&["needs an administrator: run elevate dukovo-check", "refused 3"])?;

    // The system as it was installed has nothing to warn of. SSH is off,
    // the firewall service ended with status 0 after it loaded the rules,
    // alice is the one administrator, and the snapshots are made. What
    // happens now and then (the clock, the update check) is skipped on a
    // computer that has been up for minutes, not warned.
    type_and_wait(vm, &run_check("clean", ""))?;
    let reading = [
        ended("clean")[0].clone(),
        ended("clean")[1].clone(),
        count("clean-lines", LINE, "/tmp/clean.txt"),
        count("clean-warned", WARNED, "/tmp/clean.txt"),
        count("clean-summary", "^[0-9]+ passed, 0 warnings, ", "/tmp/clean.txt"),
        count("clean-ssh", "^ssh +passwords +passed +SSH is off", "/tmp/clean.txt"),
        count(
            "clean-firewall",
            "^network +firewall on +passed +the firewall service loaded its rules",
            "/tmp/clean.txt",
        ),
        count(
            "clean-admin",
            "^accounts +administrators +passed +1 administrator: alice",
            "/tmp/clean.txt",
        ),
        count("clean-alerts", "^alerts +reach +passed +no channel is set up", "/tmp/clean.txt"),
        count("clean-snapshots", "^backups +snapshots +passed ", "/tmp/clean.txt"),
    ];
    answer_prompt_within(vm, "$", &format!("clear; {}", reading.join("; ")), 60)?;
    vm.expect_screen(&[
        "clean 0",
        "clean-lines 17",
        "clean-warned 0",
        "clean-summary 1",
        "clean-ssh 1",
        "clean-firewall 1",
        "clean-admin 1",
        "clean-alerts 1",
        "clean-snapshots 1",
    ])?;
    let took = took_seconds(&vm.screen()?, "clean")?;
    eprintln!(
        "          dukovo-check: {took} s{}",
        if vm.kvm { "" } else { " (no KVM, so not held to the budget)" }
    );
    if vm.kvm && took > BUDGET_S {
        return Err(format!("dukovo-check took {took} s, not within {BUDGET_S} s"));
    }

    // alice is in admins, so she reads the kept result: what the run
    // showed, after its first line, which she is told by the run alone. The
    // file is for the group admins, root's, and nobody else.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; dukovo-check --last > /tmp/last.txt; echo $? > /tmp/last.status; \
             tail -n +2 /tmp/clean.txt > /tmp/a.txt; tail -n +2 /tmp/last.txt > /tmp/b.txt; \
             echo \"last $(cat /tmp/last.status)\"; {}; {}; \
             [ \"$(cat /tmp/a.txt)\" = \"$(cat /tmp/b.txt)\" ] && echo \"same $((40 + 2))\"; \
             echo \"mode $(ls -ln {RESULT} | grep -c -E '^-rw-r----- +[0-9]+ 0 10 ')\"",
            count("last-heading", "^The last check ended ", "/tmp/last.txt"),
            count("last-lines", LINE, "/tmp/last.txt"),
        ),
    )?;
    vm.expect_screen(&["last 0", "last-heading 1", "last-lines 16", "same 42", "mode 1"])?;

    // SSH on, as it was meant to be left: keys alone sign in. The port is
    // one that other machines reach now.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {}; for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do {} | \
             grep -q 'tcp 22' && break; sleep 1; done; true",
            as_root("service sshd enable"),
            as_root("firewall"),
        ),
    )?;
    type_and_wait(vm, &run_check("keys", ""))?;
    let reading = [
        ended("keys")[0].clone(),
        count("keys-warned", WARNED, "/tmp/keys.txt"),
        count(
            "keys-ssh",
            "^ssh +passwords +passed +only keys sign in over SSH",
            "/tmp/keys.txt",
        ),
        count(
            "keys-port",
            "^network +reachable +passed +from other machines: .*tcp 22 .sshd.",
            "/tmp/keys.txt",
        ),
    ];
    answer_prompt_within(vm, "$", &format!("clear; {}", reading.join("; ")), 60)?;
    vm.expect_screen(&["keys 0", "keys-warned 0", "keys-ssh 1", "keys-port 1"])?;

    // Passwords on, for everyone: SEC-110, and SEC-111 with it, since no
    // list says who may sign in. Each line has its ID and what to do, and
    // the status is 2.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {}; {}; true",
            as_root(&format!("sh -c 'echo PasswordAuthentication yes > {SSHD_CONFIG}'")),
            as_root("service sshd reload"),
        ),
    )?;
    type_and_wait(vm, &run_check("passwords", ""))?;
    let reading = [
        ended("passwords")[0].clone(),
        count(
            "passwords-line",
            "^ssh +passwords +warning +passwords sign in over SSH",
            "/tmp/passwords.txt",
        ),
        count(
            "passwords-anyone",
            "^ssh +who may sign in +warning +any account may sign in",
            "/tmp/passwords.txt",
        ),
        count("passwords-110", "^ +SEC-110: Add your SSH key", "/tmp/passwords.txt"),
        count("passwords-111", "^ +SEC-111: ", "/tmp/passwords.txt"),
        count("passwords-summary", "^[0-9]+ passed, 2 warnings, ", "/tmp/passwords.txt"),
    ];
    answer_prompt_within(vm, "$", &format!("clear; {}", reading.join("; ")), 60)?;
    vm.expect_screen(&[
        "passwords 2",
        "passwords-line 1",
        "passwords-anyone 1",
        "passwords-110 1",
        "passwords-111 1",
        "passwords-summary 1",
    ])?;

    // The log has one line for the run, at the level of its status, with
    // the IDs of what it found.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {} | grep -c 'check ended with status 2: .*(SEC-110, SEC-111)' | \
             sed 's/^1$/logged/'",
            as_root("dmesg")
        ),
    )?;
    vm.expect_screen(&["logged"])?;

    // --json prints what the file holds, whole, and --last --json reads
    // the same back, as alice.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {} > /tmp/run.json; echo $? > /tmp/run.status; \
             dukovo-check --last --json > /tmp/last.json; echo $? > /tmp/last-json.status; true",
            as_root("dukovo-check --json"),
        ),
    )?;
    let reading = [
        "echo \"json $(cat /tmp/run.status) $(cat /tmp/last-json.status)\"".to_string(),
        "[ \"$(cat /tmp/run.json)\" = \"$(cat /tmp/last.json)\" ] && echo \"same $((40 + 2))\""
            .to_string(),
        count("json-open", "^\\{$", "/tmp/run.json"),
        count("json-close", "^\\}$", "/tmp/run.json"),
        count("json-version", "^  \"version\": 1,$", "/tmp/run.json"),
        count(
            "json-checked",
            "^  \"checked\": \"[0-9-]+T[0-9:]+Z\",$",
            "/tmp/run.json",
        ),
        count("json-release", "^  \"release\": \"[^\"]+\",$", "/tmp/run.json"),
        count("json-warnings", "^      \"result\": \"warning\",$", "/tmp/run.json"),
        count("json-id", "^      \"id\": \"SEC-11[01]\",$", "/tmp/run.json"),
        count("json-counts", "^    \"(warning\": 2|failed\": 0),$", "/tmp/run.json"),
    ];
    answer_prompt_within(vm, "$", &format!("clear; {}", reading.join("; ")), 60)?;
    vm.expect_screen(&[
        "json 2 2",
        "same 42",
        "json-open 1",
        "json-close 1",
        "json-version 1",
        "json-checked 1",
        "json-release 1",
        "json-warnings 2",
        "json-id 2",
        "json-counts 2",
    ])?;

    // Passwords off again: the warnings go, and the status is 0.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {}; {}; true",
            as_root(&format!("sh -c 'echo PasswordAuthentication no > {SSHD_CONFIG}'")),
            as_root("service sshd reload"),
        ),
    )?;
    type_and_wait(vm, &run_check("again", ""))?;
    let reading = [
        ended("again")[0].clone(),
        count("again-warned", WARNED, "/tmp/again.txt"),
        count(
            "again-ssh",
            "^ssh +passwords +passed +only keys sign in over SSH",
            "/tmp/again.txt",
        ),
        count("again-summary", "^[0-9]+ passed, 0 warnings, ", "/tmp/again.txt"),
    ];
    answer_prompt_within(vm, "$", &format!("clear; {}", reading.join("; ")), 60)?;
    vm.expect_screen(&["again 0", "again-warned 0", "again-ssh 1", "again-summary 1"])?;

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
            as_root("dukovo-check --json"),
            "echo 'queen of hearts' | elevate -S dukovo-check --json"
        );
    }

    #[test]
    fn a_count_says_its_label_and_the_number_of_matching_lines() {
        assert_eq!(
            count("lines", "^ssh ", "/tmp/c"),
            "echo \"lines $(grep -c -E '^ssh ' /tmp/c)\""
        );
    }

    #[test]
    fn a_run_keeps_what_it_printed_its_status_and_its_seconds() {
        let command = run_check("clean", "");
        assert!(command.contains("elevate -S dukovo-check > /tmp/clean.txt;"), "{command}");
        assert!(command.contains("echo $? > /tmp/clean.status;"), "{command}");
        assert!(command.ends_with("echo $((e - s)) > /tmp/clean.took"), "{command}");
        assert!(run_check("x", "--json").contains("elevate -S dukovo-check --json > /tmp/x.txt"));
    }

    #[test]
    fn reads_the_seconds_a_run_took_from_the_screen() {
        let screen = vec!["clean 0".to_string(), "  clean-took 3".to_string()];
        assert_eq!(took_seconds(&screen, "clean"), Ok(3));
        assert!(took_seconds(&screen, "keys").is_err());
        assert!(took_seconds(&["clean-took soon".to_string()], "clean").is_err());
    }
}
