// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An update check on a computer whose clock is a year ahead (ADR 0536).
//! timed is given no servers and the clock is set as QEMU's `-rtc base=`
//! started it, so that it stays a year ahead, and the test update server
//! (`update-online`'s) serves the signed metadata it published, whose
//! timestamp is good for a day.
//! *sysupdate check* refuses the expired timestamp with a message that
//! names when it expired and what the computer's clock says, and says to
//! set the clock; the server is asked for no file of a release, and
//! *sysupdate status* is as it was.

use super::super::*;
use super::update_online::Server;
use super::update_stage::{SLOT_STATE, power_off, run, seen_through, started};

pub const CASE: Case = Case {
    name: "update-clock-ahead",
    about: "an update check on a computer whose clock is a year ahead, and which timed does not set: sysupdate check refuses the server's expired timestamp, says when it expired and what the clock says and that the clock is to be set, asks for no file of a release, and changes nothing (ADR 0536)",
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
        RTC_A_YEAR_AHEAD,
    ],
    script: update_clock_ahead,
    disks: &[],
    second: None,
};

/// Starts the machine with a clock that timed leaves alone, `ahead`
/// seconds past the host's (before it, when negative), and the update
/// server `server` followed, and checks that the clock is in that year.
///
/// timed is told to ask no server, and said so in its log, before
/// anything else is done: a clock that a time server had set would test
/// nothing. A server may have answered timed before it was told, so
/// while it is stopped the clock is set `ahead` again, as QEMU started
/// it.
pub(crate) fn start_with_a_wrong_clock(
    vm: &mut Vm,
    server: &Server,
    ahead: i64,
) -> Result<(), String> {
    started(vm, 'a')?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    let clock = clock_text(host_time_ahead(ahead));
    run(
        vm,
        &format!(
            "printf 'servers =\\nfallback =\\n' > /system/settings/timed && service timed stop \
             > /dev/null && TZ=UTC timedatectl set-time '{clock}' && service timed start > \
             /dev/null; until log timed | grep -q 'NTS servers none'; do sleep 1; done; echo \
             \"told: $((6 * 7))\""
        ),
        &["told: 42"],
    )?;
    let settings = "/system/settings/sysupdate";
    run(
        vm,
        &format!(
            "printf 'server = \"http://10.0.2.2:{}/system\"\\ncanary = true\\n' > \
             {settings}/updates.toml && echo 'stable = 10' > {settings}/health.toml && echo \
             \"set: $((6 * 7))\"",
            server.port
        ),
        &["set: 42"],
    )?;
    run(
        vm,
        &format!(
            "echo \"year: $(timedatectl | grep 'Universal time' | grep -c ' {}-')\"",
            year_ahead(ahead)
        ),
        &["year: 1"],
    )
}

/// Check against metadata the clock says has expired.
fn update_clock_ahead(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let dir = Path::new("out/test").join(TEST_SERVER);
    let server = Server::start(&dir.join(TEST_SERVER_BEFORE_PULL))?;
    let year = year_ahead(A_YEAR);
    start_with_a_wrong_clock(vm, &server, A_YEAR)?;
    seen_through(vm)?;
    run(vm, &format!("{SLOT_STATE} > /tmp/before; echo \"kept: $((7 * 7))\""), &["kept: 49"])?;

    run(
        vm,
        &format!(
            "sysupdate check > /tmp/check 2>&1; echo \"check $?\"; echo \"expired: $(grep -c \
             'expired at 20' /tmp/check)\"; echo \"clock: $(grep -c 'clock says {year}-' \
             /tmp/check)\"; echo \"set: $(grep -c 'timedatectl set-time' /tmp/check)\"; echo \
             \"unchanged: $(grep -c 'nothing was changed' /tmp/check)\"; {SLOT_STATE} > \
             /tmp/after; cmp /tmp/before /tmp/after && echo \"same: $((8 * 8))\""
        ),
        &["check 1", "expired: 1", "clock: 1", "set: 1", "unchanged: 1", "same: 64"],
    )?;
    // The metadata was asked for, and no file of a release.
    if !server.file_requests().is_empty() {
        return Err(format!(
            "files were fetched from a server whose metadata had expired: {:?}",
            server.file_requests()
        ));
    }
    power_off(vm)
}
