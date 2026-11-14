// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! startup says why a start took as long as it did (ADR 0474), on an
//! installed system's disk under UEFI. A service the test adds sets up
//! for 3 seconds before it is ready. At the second start, which the
//! health check sees through, the times it keeps show each step from the
//! kernel to the services settling, the chain that set when they
//! settled, and the service among the slowest; beside the first start,
//! which set the test up, the service is new. Four copies of that
//! record stand for five good starts; the service then takes 9 seconds,
//! and the start after them settles later than usual: *startup --compare*
//! puts it beside the last good start, the step and the service that
//! grew show, and the health check says so in the log (BOOT-106).

use super::super::*;
use super::backup::script;
use super::update_stage::{power_off, restart, run, started};

pub const CASE: Case = Case {
    name: "startup",
    about: "startup says why a start took as long as it did (ADR 0474): the steps from the kernel to the services settling, the chain of services that set when they settled and the slowest, from the times the health check keeps; after five good starts, one that settles later than usual is compared with the last good start, the service that grew is named, and the health check says so (BOOT-106)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: startup,
    disks: &[],
    second: None,
};

/// What the health check goes by: it sees the start through once the
/// essential services have run for 12 seconds, which is after the
/// service of the test is ready, however long it takes.
const HEALTH: &[&str] = &["stable = 12"];

/// The service of the test: a setup step that takes `seconds` before it
/// is ready.
fn lingers(seconds: u32) -> Vec<String> {
    vec![
        "service_version = 2".into(),
        "description = \"Takes its time to set up\"".into(),
        "program = \"/system/commands/sh\"".into(),
        format!("arguments = [\"-c\", \"sleep {seconds}\"]"),
        "order = 60".into(),
        "enabled = true".into(),
        "restart = \"never\"".into(),
        "ready.when = \"ended\"".into(),
        "ready.timeout = 60".into(),
    ]
}

/// Where the service's file is.
const SERVICE: &str = "/system/settings/spark/services/lingers.toml";

/// Where the starts' records are kept.
const STARTS: &str = "/system/state/starts";

/// Types `lines` into `file` through a command, and checks the result.
fn write(vm: &mut Vm, file: &str, lines: &[&str]) -> Result<(), String> {
    run(vm, &format!("{} && echo \"written: $((6 * 7))\"", script(file, lines)), &["written: 42"])
}

/// The health check's line once a start's record has its times.
const TIMES_KEPT: &str = "sysupdate: kept the times of start ";

/// Waits for an installed system's start, for the health check to see it
/// through, and for its times, which it takes once the services settle.
fn seen_through(vm: &mut Vm) -> Result<(), String> {
    started(vm, 'a')?;
    vm.expect("is seen through")?;
    vm.expect(TIMES_KEPT)
}

fn startup(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    // The first start sets the test up, and is cut short.
    started(vm, 'a')?;
    write(vm, "/system/settings/sysupdate/health.toml", HEALTH)?;
    let slight = lingers(3);
    let slight: Vec<&str> = slight.iter().map(String::as_str).collect();
    write(vm, SERVICE, &slight)?;
    run(vm, "service check lingers && echo \"checked: $((6 * 7))\"", &["checked: 42"])?;
    restart(vm)?;

    // The second is seen through, and its record has the times.
    seen_through(vm)?;
    run(
        vm,
        "startup | head -7",
        &[
            "slot A: good",
            "kernel",
            "services",
            "nobody has signed in",
            "From the computer starting to the services settling:",
        ],
    )?;
    run(
        vm,
        "startup | grep -e 'The chain' -e 'The slowest'",
        &[
            "The chain that set when the services settled",
            "The slowest services, from started to ready:",
        ],
    )?;
    // The service that sets up for 3 seconds is among the slowest, with
    // the 3 seconds, and a little more for starting the shell, from its
    // start to its being ready. Its row in that list is the only one that
    // has the time right after the name (the columns are padded), where
    // the chain's has when it started.
    run(
        vm,
        "echo \"slowest: $(startup | grep -c '^  lingers  *[34][.][0-9] s  *started')\"",
        &["slowest: 1"],
    )?;
    run(vm, "echo \"data: $(startup --json | grep -c '\"lingers\"')\"", &["data: 1"])?;
    // The first start, which set the test up, was seen through too, and
    // has times without the service: beside it, the service is new.
    run(
        vm,
        "startup --compare | grep beside; echo \"new: $(startup --compare | grep -c '^ *lingers .* new$')\"",
        &["Start 2 beside the last good start 1:", "new: 1"],
    )?;

    // Four copies of the record, each a number later, stand for five good
    // starts; the service then takes 9 seconds. The newest record is the
    // last by name here: the numbers have one digit.
    run(
        vm,
        &format!(
            "last=$(ls {STARTS} | grep '[.]json$' | sed 's/[.]json//' | tail -1); for i in 1 2 \
             3 4; do sed \"s/^  \\\"number\\\": [0-9]*,/  \\\"number\\\": $((last + i)),/\" \
             {STARTS}/$last.json > {STARTS}/$((last + i)).json; done; \
             [ -e {STARTS}/$((last + 4)).json ] && echo \"copied: $((6 * 7))\""
        ),
        &["copied: 42"],
    )?;
    let heavy = lingers(9);
    let heavy: Vec<&str> = heavy.iter().map(String::as_str).collect();
    write(vm, SERVICE, &heavy)?;
    restart(vm)?;

    // The start after them settles later, and the health check says so.
    seen_through(vm)?;
    vm.expect("sysupdate: This start took ")?;
    vm.expect(" longer than usual: the services grew by ")?;
    vm.expect(", most in lingers (BOOT-106)")?;
    run(
        vm,
        "startup --compare | tail -15",
        &[
            "good start",
            "settled",
            "The services whose time changed most, from started to ready:",
            "lingers",
            "+6.",
        ],
    )?;
    power_off(vm)
}
