// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An update on a machine whose network stays down (ADRs 0453 and 0536).
//! The machine has a virtio network card on a QEMU hub with no other
//! port, which delivers nothing, so that netd asks for an address by DHCP
//! and never gets one. *sysupdate check* against a configured update
//! server it cannot reach fails within a time limit, with a message that
//! names the server and why it could not be reached, and *sysupdate
//! status* is as it was. An update staged from the updates' disk then
//! starts at the restart and proves itself to the health check: netd
//! running with no address is no failure of an essential service, and the
//! check does not wait for the network.

use super::super::*;
use super::crash_dump::answer_within;
use super::update_stage::{
    SLOT_STATE, UPDATE_DISKS, power_off, restart, run, seen_through, stage, started, type_command,
};

pub const CASE: Case = Case {
    name: "update-network-down",
    about: "an update on a machine whose network card delivers nothing, so that netd gets no address: sysupdate check fails within its time limit naming the update server and why it could not be reached, and changes nothing; an update staged from the updates' disk starts and proves itself, as the health check does not wait on the network (ADRs 0453 and 0536)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-netdev", "hubport,id=n0,hubid=0", "-device", "virtio-net-pci,netdev=n0"],
    script: update_network_down,
    disks: UPDATE_DISKS,
    second: None,
};

/// The server the check is told to read, which no machine answers for.
const SERVER: &str = "http://10.0.2.2:8080/system";

/// How long the check may take to fail, in seconds.
const CHECK_SECONDS: u64 = 120;

/// Check against a server out of reach, then stage, try and prove an
/// update with the network still down.
fn update_network_down(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let version = test_update_version("good");
    started(vm, 'a')?;
    // The card is there, and asks for an address that nothing gives.
    vm.expect_anywhere("netd: net0: asking for an address by DHCP")?;
    run(
        vm,
        &format!(
            "echo 'server = \"{SERVER}\"' > /system/settings/sysupdate/updates.toml && echo \
             \"set: $((6 * 7))\""
        ),
        &["set: 42"],
    )?;
    seen_through(vm)?;
    run(vm, &format!("{SLOT_STATE} > /tmp/before; echo \"kept: $((7 * 7))\""), &["kept: 49"])?;

    // The check fails within its time limit, naming the server and why.
    type_command(
        vm,
        &format!(
            "sysupdate check; echo \"check $?\"; {SLOT_STATE} > /tmp/after; cmp /tmp/before \
             /tmp/after && echo \"unchanged: $((8 * 8))\""
        ),
    )?;
    answer_within(vm, "#", "", CHECK_SECONDS)?;
    vm.expect_screen(&[SERVER, "nothing was changed", "check 1", "unchanged: 64"])
        .map_err(|err| format!("`sysupdate check`: {err}"))?;

    stage(vm, "good")?;
    restart(vm)?;
    started(vm, 'b')?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {version}) proved itself: it is now the default, and slot A \
         (Dukovo {VERSION}) the previous version"
    ))?;
    run(
        vm,
        "sysupdate status",
        &[
            &format!("Running:  slot B, Dukovo {version}"),
            &format!("Slot B:   Dukovo {version}, the default"),
            "Last health check: passed",
        ],
    )?;
    // The network was down for all of it.
    run(vm, "echo \"addresses: $(log netd | grep -c ' has 10\\.')\"", &["addresses: 0"])?;
    power_off(vm)
}
