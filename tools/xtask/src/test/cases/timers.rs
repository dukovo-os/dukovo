// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Service timers (ADR 0478), at the console as an administrator, on an
//! installed system that starts twice. Five services have a `[timer]`
//! (userspace/spark/tests/services/timers); each adds a line to a file
//! of /system/state/timers when it runs. With the clock set to a minute
//! before an hour in 2020, *service list --timers* shows them, and the
//! service with a time for each minute runs at the minute's start and the
//! one with `every = "20s"` three times in a minute. The service that
//! runs for 50 seconds every 20 does not start again while it runs: the
//! log says its time was skipped. Moved forward by two hours, the clock
//! makes each hourly timer run once, not twice. The power goes off, and
//! in the second boot, whose clock is years later, the hourly service
//! with `catch_up = true` runs once, 20 seconds after timerd starts, and
//! the one with `catch_up = false` does not.

use super::super::*;
use super::update_stage::{power_off, run, started, type_command};

/// Where the services write.
const MARKS: &str = "/system/state/timers";

/// The service files this test copies into place.
const TESTS: &str = "/system/tests/services/timers";

/// Where Spark reads service files.
const FILES: &str = "/system/settings/spark/services";

/// The services of the test.
const SERVICES: [&str; 5] = ["tick20", "slow", "minute-mark", "hourly-mark", "hourly-catch"];

/// The test.
pub const CASE: Case = Case {
    name: "timers",
    about: "service timers run services (ADR 0478): service list --timers shows each; a timer for each minute runs at the minute's start and one of every 20 seconds three times in a minute; a service that outlasts its period is not started again while it runs, and the log says the time was skipped; the clock moved two hours forward makes an hourly timer run once; after a power off, the second start, years later, runs an hourly timer with catch_up once and an hourly timer without it not at all",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: first_boot,
    disks: &[],
    second: Some(SecondBoot { cmdline: AS_IT_WAS, before: nothing_between, script: second_boot }),
};

/// The lines of the file `name` holds, shown as `name N`.
fn count(name: &str) -> String {
    format!("echo \"{name} $(wc -l < {MARKS}/{name})\"")
}

/// Keeps how many lines the file `name` holds, as `file`.
fn save(name: &str, file: &str) -> String {
    format!("wc -l < {MARKS}/{name} > {MARKS}/{file}")
}

/// How many lines the file `name` holds beyond what `file` kept.
fn grown(name: &str, file: &str) -> String {
    format!("$(( $(wc -l < {MARKS}/{name}) - $(cat {MARKS}/{file}) ))")
}

/// The growth of `name`, shown as `name grew N`.
fn show_grown(name: &str, file: &str) -> String {
    format!("echo \"{name} grew {}\"", grown(name, file))
}

fn first_boot(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    started(vm, 'a')?;

    // The clock a minute before an hour, years back: the second start's
    // clock, from the host, is later than every run.
    run(
        vm,
        "timedatectl set-time '2020-01-01 10:59:00'; mkdir -p /system/state/timers; cd /system/state/timers && touch tick20 minute-mark hourly-mark hourly-catch; cd; echo \"set $?\"",
        &["set 0"],
    )?;
    type_command(vm, &format!("cp {TESTS}/*.toml {FILES}/; echo \"copied $?\""))?;
    vm.expect_screen(&["copied 0"])?;
    // Enabled, a service with a timer is not started: timerd starts it.
    for name in SERVICES {
        run(
            vm,
            &format!("service check {name} | grep -c ': ok'; service {name} enable; echo \"{name} $?\""),
            &["1", "is not started now: timerd starts it", &format!("{name} 0")],
        )?;
    }
    run(
        vm,
        &format!("echo \"enabled-ran $(cat {MARKS}/hourly-mark {MARKS}/hourly-catch | wc -l)\""),
        &["enabled-ran 0"],
    )?;

    // Each timer is listed.
    type_command(vm, "service list --timers | grep -c -e tick20 -e slow -e minute-mark -e hourly")?;
    vm.expect_screen(&["5"])?;

    // The minute's start: its timer runs, and the 20 seconds' timer runs
    // more than once by the time 25 seconds have passed.
    vm.expect_anywhere("timerd: minute-mark: started by its timer")?;
    vm.expect_anywhere("timerd: slow: its time came while its last run still runs: skipped")?;
    run(
        vm,
        &format!("sleep 25; {}; [ $(wc -l < {MARKS}/tick20) -ge 3 ] && echo ticks-ok", count("minute-mark")),
        &["ticks-ok"],
    )?;

    // Two hours on: each hourly timer runs once for the hours it passed,
    // whether or not it had run at the first hour's start.
    run(vm, &format!("{}; {}; echo \"kept $?\"", save("hourly-mark", "mark-before"), save("hourly-catch", "catch-before")), &["kept 0"])?;
    run(vm, "timedatectl set-time '2020-01-01 13:30:00'; echo \"moved $?\"", &["moved 0"])?;
    vm.expect_anywhere("timerd: the clock moved forward by ")?;
    run(
        vm,
        &format!(
            "for i in $(seq 1 90); do [ {} -ge 1 ] && break; sleep 1; done; sleep 5; {}; {}",
            grown("hourly-mark", "mark-before"),
            show_grown("hourly-mark", "mark-before"),
            show_grown("hourly-catch", "catch-before"),
        ),
        &["hourly-mark grew 1", "hourly-catch grew 1"],
    )?;

    // What the files hold now reaches the disk for the second start.
    run(
        vm,
        &format!("{}; {}; sync && echo \"synced: $((4 * 4))\"", save("hourly-mark", "mark-before"), save("hourly-catch", "catch-before")),
        &["synced: 16"],
    )?;
    vm.monitor("commit boot")?;
    power_off(vm)
}

fn second_boot(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    started(vm, 'a')?;
    // The catch-up comes 20 seconds after timerd starts.
    vm.expect_anywhere("timerd: hourly-catch: started by its timer")?;
    run(
        vm,
        &format!(
            "sleep 10; {}; {}",
            show_grown("hourly-catch", "catch-before"),
            show_grown("hourly-mark", "mark-before"),
        ),
        &["hourly-catch grew 1", "hourly-mark grew 0"],
    )?;
    power_off(vm)
}
