// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An update over a bad network (ADR 0536). The test update server
//! (`update-online`'s) is at fault for the whole fetch, not for one
//! request: every answer is slowed to a trickle, after a wait, the first
//! request for each file of the release is answered 503, and the
//! connection of every third file's answer is dropped after some bytes of
//! the body. The fetch goes on through all of it: each file is tried again
//! within its four tries, a dropped one from where it stopped, and the
//! update stages, starts at the restart and proves itself.

use std::collections::BTreeSet;
use std::time::Duration;

use super::super::*;
use super::update_online::{Faults, Server, Tally, Trickle, expected};
use super::update_stage::{power_off, restart, run, started};

pub const CASE: Case = Case {
    name: "update-bad-network",
    about: "an update fetched from a server that, for the whole fetch, answers slowly, answers each file's first request 503 and drops the connection of every third file part way: each file is fetched within its tries, the dropped ones from where they stopped, and the update stages, starts and proves itself (ADR 0536)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: update_bad_network,
    disks: &[],
    second: None,
};

/// How the server trickles: a wait of a second before each answer, then
/// 2 KiB every 50 ms for the first 64 KiB of the body. Each wait is far
/// below the client's 30 seconds for a read.
const TRICKLE: Trickle = Trickle {
    stall: Duration::from_secs(1),
    chunk: 2048,
    pause: Duration::from_millis(50),
    until: 64 * 1024,
};

/// Where the connection of a dropped answer ends, in bytes of the body.
const DROPPED_AFTER: usize = 300;

/// Every `DROPPED_EVERY`th file answer long enough is dropped.
const DROPPED_EVERY: usize = 3;

/// The faults that last for the whole fetch.
fn bad_network() -> Faults {
    Faults {
        slow: Some(TRICKLE),
        busy: true,
        drop_every: Some((DROPPED_EVERY, DROPPED_AFTER)),
        ..Faults::default()
    }
}

/// Fetch through the faults, stage, start, prove.
fn update_bad_network(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let dir = Path::new("out/test").join(TEST_SERVER);
    let bridge = test_server_version("bridge");
    let files = expected(&dir, "bridge")?;
    let server = Server::start(&dir.join(TEST_SERVER_BEFORE_PULL))?;

    started(vm, 'a')?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
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

    *server.faults() = bad_network();
    let before = server.file_requests().len();
    run(
        vm,
        "sysupdate stage",
        &[
            &format!("fetching {files} files of Dukovo {bridge}"),
            &format!("sysupdate: staged Dukovo {bridge} in slot B: it starts at the next restart"),
        ],
    )?;
    faults_were_met(&server, before, files)?;

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

/// Checks that the server did what it was told to, and that the fetch
/// took exactly the `files` the slot lacked, each once complete: a 503 for
/// each, a drop, a fetch that went on from where the drop was, and answers
/// that were slow.
fn faults_were_met(server: &Server, before: usize, files: usize) -> Result<(), String> {
    let requests = server.file_requests().split_off(before);
    let fetched: BTreeSet<&str> = requests.iter().map(|(path, _)| path.as_str()).collect();
    if fetched.len() != files {
        return Err(format!("{} files were asked for, not {files}: {fetched:?}", fetched.len()));
    }
    let Tally { busy, dropped, slowed, .. } = server.tally();
    if busy != files {
        return Err(format!("{busy} first requests were answered 503, not {files}"));
    }
    if dropped == 0 || slowed == 0 {
        return Err(format!("{dropped} connections were dropped, {slowed} answers were slow"));
    }
    let resumed = format!("bytes={DROPPED_AFTER}-");
    if !requests.iter().any(|(_, range)| range.as_deref() == Some(resumed.as_str())) {
        return Err(format!("no fetch went on from byte {DROPPED_AFTER}: {requests:?}"));
    }
    Ok(())
}
