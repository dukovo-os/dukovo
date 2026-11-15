// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The machine's part of docs/runbooks/security-advisory.md (ADR 0534),
//! under UEFI on an installed system's disk with no network card, so
//! that the update server cannot be read. The runbook's host steps are
//! covered where they run: *ob security check* and *advisory new* by the
//! unit tests of ob and of system-release, the signing of the advisories
//! with a channel or alone, and the publishing of them by the update
//! server, by system-release's tests, and the machine's reading of the
//! signed index and advisories through its TUF client by systools' own
//! test, which signs a repository.
//!
//! *update advisories* says that the advisories cannot be read, in a
//! line that starts *update:*, and ends with status 3, as a check that
//! found nothing and could not look does. Its *--json* document names the
//! part that could not be checked, and still has the *system* and *apps*
//! lists. A word it does not know is a usage error, status 2. *update
//! check* has its Security part, which says the same, and ends with
//! status 0 as the other parts do.

use super::super::*;
use super::update_stage::{power_off, run, started};

pub const CASE: Case = Case {
    name: "runbook-security-advisory",
    about: "the machine's part of the security advisory runbook (ADR 0534), with no network: update advisories says that the advisories cannot be read and ends with status 3, its JSON names the part that could not be checked and has the system and apps lists, an unknown word is a usage error with status 2, and update check has its Security part",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: runbook_security_advisory,
    disks: &[],
    second: None,
};

/// Where the JSON document is kept for the checks of its members.
const DOCUMENT: &str = "/tmp/advisories.json";

/// A command that says how many lines of [`DOCUMENT`] match `pattern`, as
/// `label 1`: the typed command holds no such text, so the screen shows it
/// only when the command has run.
fn count(label: &str, pattern: &str) -> String {
    format!("echo \"{label} $(grep -c '{pattern}' {DOCUMENT})\"")
}

/// The advisories with no server to ask, in text and in JSON, a usage
/// error, and the Security part of a check.
fn runbook_security_advisory(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    started(vm, 'a')?;
    run(
        vm,
        "update advisories; echo \"advisories $?\"",
        &["update: the advisories cannot be read", "advisories 3"],
    )?;
    run(
        vm,
        &format!(
            "update advisories --json > {DOCUMENT}; echo \"json $?\"; {}; {}",
            count("document", "format.*system.*apps.*unchecked"),
            count("unchecked", "unchecked.*item.*system"),
        ),
        &["json 3", "document 1", "unchecked 1"],
    )?;
    run(vm, "update advisories --bogus; echo \"usage $?\"", &["usage: update", "usage 2"])?;
    run(
        vm,
        "update check; echo \"check $?\"",
        &["Security:", "update: the advisories cannot be read", "check 0"],
    )?;
    power_off(vm)
}
