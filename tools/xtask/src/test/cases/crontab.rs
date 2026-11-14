// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A person's jobs: crontab(1) and at(1) (ADR 0478). Alice signs in as
//! the owner. A crontab line that is wrong is refused at its column. A
//! line for each minute runs while she is signed in, and an `at now + 1
//! minute` job runs once and leaves the queue; *atq* lists a job and
//! *atrm* removes it. With *crontab --keep-running off*, which is the
//! system's default, the line does not run once she signs out, and
//! *crontab --status* says why, and her next sign-in says how many did
//! not run; with it on the line runs while she is signed out. An
//! administrator who sets *user set --jobs-signed-out never* for her
//! stops it again, and her sign-in says so. Root's crontab runs too, as
//! root without the bypass right, as root's sessions run, so it cannot
//! write in her home: *crontab -u root --status* shows its output. The
//! power button ends it.

use super::super::*;

/// Waits up to 100 seconds for the file `file` to hold something.
fn wait_for(file: &str) -> String {
    format!("for i in $(seq 1 100); do [ -s {file} ] && break; sleep 1; done")
}

/// Types `command`, which waits up to 100 seconds for a job, at alice's
/// prompt; waits for it to end, which a screen check alone does not wait
/// as long for; and checks that the screen then shows each of `shown`.
fn waited(vm: &mut Vm, command: &str, shown: &[&str]) -> Result<(), String> {
    answer_prompt(vm, "$", command)?;
    answer_prompt_within(vm, "$", "", 130)?;
    vm.expect_screen(shown)
}

/// Checks that the log, from alice's last sign-out to her sign-in after
/// it, says her line did not run because `why`, and shows no job of hers
/// started. A line that runs once she is back, before the test looks, is
/// not counted, as a count of the file's lines would count it.
fn no_run_while_out(vm: &Vm, why: &str) -> Result<(), String> {
    let log = vm.log();
    let out = log.rfind("authd: alice signed out on pts/").ok_or("alice never signed out")?;
    let back = log[out..].find("authd: alice signed in on pts/").map_or(log.len(), |at| out + at);
    let away = &log[out..back];
    if !away.contains(&format!("crontab alice (line 1): did not run: {why}")) {
        return Err(format!("while alice was signed out, the log does not say her line did not run: {why}"));
    }
    if away.contains("authd: alice's job crontab alice, for process ") {
        return Err("alice's line ran while she was signed out".into());
    }
    Ok(())
}

/// Signs alice out and in again, `seconds` apart.
fn out_and_in(vm: &mut Vm, seconds: u32) -> Result<(), String> {
    answer_prompt(vm, "$", "clear; exit")?;
    vm.expect_anywhere("authd: alice signed out on pts/")?;
    vm.expect_screen(&["Name:"])?;
    // Left at the sign-in, the jobs of the minute come while she is out.
    std::thread::sleep(Duration::from_secs(u64::from(seconds)));
    vm.type_text("alice\n")?;
    answer_prompt(vm, "Password:", "queen of hearts")?;
    answer_prompt(vm, "$", "")
}

/// The test.
pub const CASE: Case = Case {
    name: "crontab",
    about: "a person's jobs (ADR 0478): a wrong crontab line is refused at its column; a line for each minute runs while alice is signed in; an at job runs once and leaves the queue, atq lists a job and atrm removes it; with crontab --keep-running off the line does not run while she is signed out, and crontab --status says why; with it on it runs while she is signed out; user set --jobs-signed-out never stops it again; root's crontab runs",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: crontab,
    disks: &[],
    second: None,
};

fn crontab(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("timerd: keeping time")?;

    // A wrong line is refused at its column, and nothing is installed.
    answer_prompt(vm, "$", "clear; printf '61 * * * * true\\n' | crontab -; echo \"refused $?\"")?;
    vm.expect_screen(&["line 1, column 1", "refused 1"])?;
    answer_prompt(vm, "$", "clear; crontab -l; echo \"listed $?\"")?;
    vm.expect_screen(&["listed 1"])?;

    // A line for each minute runs while she is signed in.
    answer_prompt(
        vm,
        "$",
        "clear; printf '* * * * * echo alice-ran >> ~/cron.out\\n' | crontab -; crontab -l; echo \
         \"listed $?\"",
    )?;
    vm.expect_screen(&["* * * * * echo alice-ran >> ~/cron.out", "listed 0"])?;
    waited(vm, &format!("clear; {}; cat ~/cron.out", wait_for("~/cron.out")), &["alice-ran"])?;

    // An at job runs once, and leaves the queue.
    answer_prompt(
        vm,
        "$",
        "clear; echo 'echo at-ran >> ~/at.out' | at now + 1 minute; echo \"queued $(atq | wc -l)\"",
    )?;
    vm.expect_screen(&["job 1 at ", "queued 1"])?;
    waited(
        vm,
        &format!("clear; {}; cat ~/at.out; echo \"queued $(atq | wc -l)\"", wait_for("~/at.out")),
        &["at-ran", "queued 0"],
    )?;
    answer_prompt(
        vm,
        "$",
        "clear; echo true | at now + 1 hour; atq | wc -l; atrm 2; echo \"left $(atq | wc -l)\"",
    )?;
    vm.expect_screen(&["job 2 at ", "left 0"])?;

    // Off, which is the system's default, she is signed out and the line
    // does not run; the status says why.
    answer_prompt(vm, "$", "clear; crontab --keep-running off")?;
    vm.expect_screen(&["do not run while you are signed out"])?;
    out_and_in(vm, 70)?;
    vm.expect_screen(&[
        "of yours did not run while you were signed out",
        "crontab --keep-running on lets them run then.",
    ])?;
    no_run_while_out(vm, "its owner is signed out")?;
    answer_prompt(
        vm,
        "$",
        "clear; crontab --status | grep -q 'is signed out' && echo \"status says why\"",
    )?;
    vm.expect_screen(&["status says why"])?;

    // On, it runs while she is signed out.
    answer_prompt(vm, "$", "clear; crontab --keep-running on; wc -l < ~/cron.out > ~/before")?;
    vm.expect_screen(&["run while you are signed out"])?;
    out_and_in(vm, 70)?;
    answer_prompt(
        vm,
        "$",
        "clear; [ $(( $(wc -l < ~/cron.out) - $(cat ~/before) )) -ge 1 ] && echo \"ran signed out\"",
    )?;
    vm.expect_screen(&["ran signed out"])?;

    // An administrator bars it for her: signed out again, it does not run.
    answer_prompt(
        vm,
        "$",
        "clear; elevate user set --jobs-signed-out never alice; echo \"barred $?\"",
    )?;
    answer_prompt(vm, "alice:", "queen of hearts")?;
    vm.expect_screen(&["barred 0"])?;
    out_and_in(vm, 70)?;
    vm.expect_screen(&["An administrator barred your jobs from running then."])?;
    no_run_while_out(vm, "an administrator barred its owner's jobs")?;
    answer_prompt(vm, "$", "clear; crontab --keep-running")?;
    vm.expect_screen(&["An administrator barred it for your account."])?;

    // Root's crontab runs, and keeps what its line printed. The second
    // elevate is in the same session, so it asks for no password.
    answer_prompt(
        vm,
        "$",
        "clear; printf '* * * * * echo root-ran\\n' > root.cron; elevate crontab -u root \
         root.cron; echo \"root $?\"",
    )?;
    answer_prompt(vm, "root.cron:", "queen of hearts")?;
    vm.expect_screen(&["root 0"])?;
    vm.expect("timerd: crontab root (line 1): ended with status 0")?;
    answer_prompt(vm, "$", "clear; elevate crontab -u root --status")?;
    vm.expect_screen(&["root-ran", "status 0"])?;
    let log = vm.log();
    for line in [
        "authd: alice's job crontab alice, for process ",
        "while they are signed out",
        "crontab alice (line 1): did not run: its owner is signed out",
        "authd: root's job crontab root, for process ",
        "authd: changed the account alice (1000): its jobs never run while it is signed out",
    ] {
        if !log.contains(line) {
            return Err(format!("the log does not show \"{line}\""));
        }
    }

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
