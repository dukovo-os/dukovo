// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An update on a computer whose clock is a year behind (ADR 0536).
//! timed is given no servers, so the clock stays as QEMU's `-rtc base=`
//! sets it. TUF has no rule for a clock behind: the metadata is not too
//! new for it, so *sysupdate check* works, and the update stages from the
//! test update server (`update-online`'s), starts at the restart and
//! proves itself. A timestamp older than the one the machine read is
//! still refused, by its version.

use super::super::*;
use super::update_clock_ahead::start_with_a_wrong_clock;
use super::update_online::{Faults, Server, expected};
use super::update_stage::{power_off, restart, run, started};

pub const CASE: Case = Case {
    name: "update-clock-behind",
    about: "an update on a computer whose clock is a year behind, and which timed does not set: sysupdate check works, as TUF has no rule for a clock behind, a replayed older timestamp is still refused by its version, and the update stages, starts and proves itself (ADR 0536)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[
        "-netdev",
        "user,id=n0",
        "-device",
        "virtio-net-pci,netdev=n0",
        "-rtc",
        RTC_A_YEAR_BEHIND,
    ],
    script: update_clock_behind,
    disks: &[],
    second: None,
};

/// Check, refuse a replay, stage, start, prove.
fn update_clock_behind(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let dir = Path::new("out/test").join(TEST_SERVER);
    let (bridge, year) = (test_server_version("bridge"), test_server_version("year"));
    let bridge_files = expected(&dir, "bridge")?;
    let stale = std::fs::read(dir.join("stale-timestamp.json")).map_err(|err| err.to_string())?;
    let server = Server::start(&dir.join(TEST_SERVER_BEFORE_PULL))?;
    start_with_a_wrong_clock(vm, &server, year_ahead(-A_YEAR))?;

    run(
        vm,
        "sysupdate check",
        &[
            &format!(
                "sysupdate: the stable channel offers Dukovo {year}; this system runs Dukovo {VERSION}"
            ),
            &format!("Dukovo {bridge} needs {bridge_files} files this system lacks"),
        ],
    )?;
    // What the machine read is not taken back for an older timestamp.
    server.faults().stale = Some(stale);
    run(vm, "sysupdate check; echo \"check $?\"", &["is older than the trusted", "check 1"])?;
    *server.faults() = Faults::default();

    run(
        vm,
        "sysupdate stage",
        &[
            &format!("fetching {bridge_files} files of Dukovo {bridge}"),
            &format!("sysupdate: staged Dukovo {bridge} in slot B: it starts at the next restart"),
        ],
    )?;
    restart(vm)?;
    started(vm, 'b')?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {bridge}) proved itself: it is now the default"
    ))?;
    run(
        vm,
        "sysupdate status",
        &[&format!("Running:  slot B, Dukovo {bridge}"), "Last health check: passed"],
    )?;
    power_off(vm)
}
