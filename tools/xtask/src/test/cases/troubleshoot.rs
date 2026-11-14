// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! troubleshoot names the likely cause of a start that failed (ADR 0474),
//! on an installed system's disk under UEFI. The health check is told
//! that a service the test adds is essential, and that service ends at
//! once with a message that says the disk is full. At the restart the
//! health check fails the start, and the failure notice it leaves on the
//! sign-in screens names the likely cause, BOOT-102, with its fix.
//! *troubleshoot* says it too, with the page of the support center and,
//! with `--json`, as data. Its signature for a full disk, BOOT-104,
//! matches only when the disk is full as well as the message in the log:
//! with the disk as it is the message alone matches nothing, and with the
//! disk's fullness changed in the facts *troubleshoot* saved, BOOT-104
//! is also possible after BOOT-102. A newer file of signatures kept in
//! the system's state is the one in use, and an older one is not.

use super::super::*;
use super::backup::script;
use super::install_uefi::wait_for_screen;
use super::update_stage::{power_off, restart, run, started};

pub const CASE: Case = Case {
    name: "troubleshoot",
    about: "troubleshoot names the likely cause of a failed start (ADR 0474): a service the health check needs ends at once with a full-disk message, the start fails, and the failure notice and troubleshoot say BOOT-102 with its fix and page, as data too; BOOT-104 does not match on the message alone and is the second possible cause once the facts say the disk is full; a newer file of signatures kept in the state is the one in use, an older one is not",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: troubleshoot,
    disks: &[],
    second: None,
};

/// What the health check goes by: a service of the test's is essential.
const HEALTH: &[&str] = &["essential = [\"logd\", \"trouble\"]", "stable = 5"];

/// The service that does not work: it says the disk is full, and ends
/// with a status that is not 0, and is not started again.
const TROUBLE: &[&str] = &[
    "service_version = 1",
    "description = \"Ends at once, as a service that cannot write does\"",
    "program = \"/system/commands/sh\"",
    "arguments = [\"-c\", \"echo trouble: cannot write: No space left on device; exit 3\"]",
    "order = 60",
    "enabled = true",
    "restart = \"never\"",
];

/// A file of signatures with a version, and one signature for BOOT-102.
fn signatures(version: u32, cause: &str) -> Vec<String> {
    vec![
        "format = 1".into(),
        format!("version = {version}"),
        "[[signature]]".into(),
        "id = \"BOOT-102\"".into(),
        format!("cause = \"{cause}.\""),
        "fix = \"Read what the file says.\"".into(),
        "match = [{ start = \"failed\", error = \"BOOT-102\" }]".into(),
    ]
}

/// The notice the failed start leaves, once it is there.
const NOTICE: &str = "/system/live/sysupdate/failure";

/// Where the facts of the failed start are kept, and where the same
/// facts with the disk full are.
const FACTS: &str = "/tmp/facts";
const FULL: &str = "/tmp/full";

/// Where a newer file of signatures is kept.
const KEPT: &str = "/system/state/troubleshoot/signatures.toml";

/// Types `lines` into `file` through a command, and checks the result.
fn write(vm: &mut Vm, file: &str, lines: &[&str]) -> Result<(), String> {
    run(vm, &format!("{} && echo \"written: $((6 * 7))\"", script(file, lines)), &["written: 42"])
}

fn troubleshoot(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    started(vm, 'a')?;
    write(vm, "/system/settings/sysupdate/health.toml", HEALTH)?;
    write(vm, "/system/settings/spark/services/trouble.toml", TROUBLE)?;
    run(vm, "service check trouble && echo \"checked: $((6 * 7))\"", &["checked: 42"])?;

    // The restart: the service ends at once, and the start fails its
    // health check, as no update's try, so it goes on.
    restart(vm)?;
    started(vm, 'a')?;
    // The health check may fail the start before the screen shows tty1.
    vm.expect_anywhere("failed its health check: ")?;
    // The notice of the failed start shows on the screen's last line
    // until a key is pressed, which would hide the prompt. It comes some
    // seconds after the line above, so wait for its own text, then press
    // Enter.
    wait_for_screen(vm, "sysupdate: Start ", 60)?;
    vm.press(&["ret"])?;
    run(
        vm,
        &format!(
            "i=0; while [ ! -e {NOTICE} ] && [ $i -lt 60 ]; do sleep 1; i=$((i + 1)); done; cat \
             {NOTICE}"
        ),
        &[
            "failed its health check: ",
            "(BOOT-102)",
            "Likely cause: An essential service ended",
            "More: changes --failed",
        ],
    )?;

    // troubleshoot says the same, with the page, and as data.
    run(
        vm,
        "troubleshoot; echo \"status: $?\"",
        &[
            "Likely cause: An essential service ended",
            "Fix: The start's message names the service",
            "/errors/BOOT-102.html",
            "status: 0",
        ],
    )?;
    run(
        vm,
        "echo \"data: $(troubleshoot --json | grep -c '\"id\":\"BOOT-102\"')\"; echo \"about: \
         $(troubleshoot --about | head -1)\"",
        &["data: 1", "about: Signatures: version 1, from the file this release ships."],
    )?;

    // The message of a full disk matches nothing alone: the disk is not
    // full.
    run(vm, "echo \"alone: $(troubleshoot --all | grep -c BOOT-104)\"", &["alone: 0"])?;
    // With the facts saved and the disk's fullness changed, it is the
    // second possible cause, after the one the start's own error names.
    run(
        vm,
        &format!(
            "troubleshoot --save-facts {FACTS} > /dev/null; mkdir {FULL} && sed 's/\"percent\": \
             [0-9]*/\"percent\": 100/' {FACTS}/facts.json > {FULL}/facts.json && echo \"planted: \
             $((6 * 7))\""
        ),
        &["planted: 42"],
    )?;
    run(
        vm,
        &format!("troubleshoot --facts {FULL}; echo \"status: $?\""),
        &[
            "Likely cause: An essential service ended",
            "Also possible: The disk that holds the system is full (BOOT-104)",
            "status: 0",
        ],
    )?;

    // A file of signatures with a version that is not newer than the
    // shipped one's is not used; a newer one is.
    run(vm, "mkdir -p /system/state/troubleshoot && echo \"made: $((6 * 7))\"", &["made: 42"])?;
    let older = signatures(1, "The older file is not used");
    let older: Vec<&str> = older.iter().map(String::as_str).collect();
    write(vm, KEPT, &older)?;
    run(
        vm,
        "echo \"older: $(troubleshoot | grep -c 'older file')\"; troubleshoot --about | head -1",
        &["older: 0", "Signatures: version 1, from the file this release ships."],
    )?;
    let newer = signatures(99, "The newer file says an essential service ended");
    let newer: Vec<&str> = newer.iter().map(String::as_str).collect();
    write(vm, KEPT, &newer)?;
    run(
        vm,
        "troubleshoot; troubleshoot --about | head -1",
        &[
            "Likely cause: The newer file says an essential service ended (BOOT-102)",
            "Fix: Read what the file says.",
            "Signatures: version 99, from a newer file the update check kept.",
        ],
    )?;
    power_off(vm)
}
