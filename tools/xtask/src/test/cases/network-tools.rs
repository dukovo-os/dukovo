// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The administrator's network tools between two machines on a cable
//! (ADRs 0341 and 0342), each from the test image, with nettooltest
//! (userspace/posix/nettooltest.c): machine a's card is an e1000e,
//! whose link QEMU gives at 1000 Mb/s full duplex, and b's a virtio
//! card. b serves a name server for tools.test and WHOIS servers that
//! answer as IANA's, a registry's and a registrar's. nic(8) must show
//! a's speed, duplex, driver, model, rings, offloads and counts, and b's
//! link without a speed; nc(1) must carry TCP both ways by IPv4 and
//! IPv6, and UDP by both, its listener peeking at the first datagram
//! (MSG_PEEK), and find b's open port; iperf3(1) must measure TCP and
//! UDP to b's server, by IPv6 and the other way, and in JSON; dig(1) and
//! host(1) must read b's records, ask again over TCP for an answer too
//! long for UDP, and say when no server answers, and host must reach
//! b's server through resolved(8); whois(1) must follow IANA's referral
//! to the registry and the registrar's, through names resolved finds,
//! and ask ARIN's server for networks; and openssl(1) must make keys, a
//! certificate and digests, and b's s_client reach a's s_server over
//! TLS 1.3.

use super::super::*;

/// What machine b runs: the same program, on its virtio card at the
/// cable's other end.
const PEER_CMDLINE: &str =
    "lockdep -- --service-dir /system/tests/services/tools --posix --network /system/tests/posix/nettooltest b";

pub const CASE: Case = Case {
    name: "network-tools",
    about: "two machines on a cable: nic shows an e1000e's 1000 Mb/s full-duplex link, driver and rings and a virtio card's link; nc carries TCP and UDP both ways; iperf3 measures b; dig and host read b's name server and host reaches it through resolved; whois follows IANA's referrals; openssl makes keys and certificates, and b's s_client reaches a's s_server over TLS 1.3 (ADRs 0341-0342)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep -- --service-dir /system/tests/services/tools --posix --network /system/tests/posix/nettooltest a",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-netdev",
        CABLE[0],
        "-device",
        "e1000e,netdev=cable,mac=52:54:00:12:34:a0",
        PEER,
        PEER_CMDLINE,
        "-netdev",
        CABLE[1],
        "-device",
        "virtio-net-pci,netdev=cable,mac=52:54:00:12:34:a1",
    ],
    script: network_tools,
    disks: &[],
    second: None,
};

/// What nettooltest a says once each part has passed.
const PARTS: &[&str] = &[
    "nettooltest: nic showed the e1000e's link, driver, rings, offloads and counts",
    "nettooltest: nc carried TCP and UDP both ways, by IPv4 and IPv6, and found b's open port",
    "nettooltest: iperf3 measured TCP and UDP to b, by IPv4 and IPv6, both ways",
    "nettooltest: dig and host asked b's name server, and through resolved",
    "nettooltest: whois followed IANA's referrals to the registrar, and asked ARIN for networks",
    "nettooltest: openssl made keys and a certificate, and b's s_client reached a's s_server over TLS 1.3",
];

fn network_tools(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("Net: net0 is an Intel 82574L network card")?;
    vm.expect("nettooltest: waiting for b on port 7000")?;
    vm.boot_peer()?;
    {
        let b = vm.peer()?;
        b.expect("Boot complete.")?;
        b.expect_anywhere("nettooltest: nic showed the virtio card's link, which says no speed")?;
        b.expect_anywhere("nettooltest: connected to a")?;
    }
    let code = program_line(vm, "(/system/tests/posix/nettooltest) exited with code ")?;
    passed(vm, "a", &code)?;
    let log = vm.log();
    if let Some(part) = PARTS.iter().find(|part| !log.contains(*part)) {
        return Err(format!("nettooltest a never said \"{part}\""));
    }
    let b = vm.peer()?;
    let code = program_line(b, "(/system/tests/posix/nettooltest) exited with code ")?;
    passed(b, "b", &code)?;
    if !b.log().contains("nettooltest: the name server answered big.tools.test, type 16, with 9 records") {
        return Err("b's name server never gave big.tools.test's nine records over TCP".into());
    }
    Ok(())
}

/// Checks that the nettooltest of machine `who` passed, by its log and
/// its exit code. What failed comes with what nettooltest said about it,
/// such as how a command ended, as the log's tail may not hold it.
fn passed(vm: &Vm, who: &str, code: &str) -> Result<(), String> {
    let log = vm.log();
    let failed: Vec<&str> = log
        .lines()
        .filter(|line| {
            line.contains("nettooltest: FAILED")
                || (line.contains("nettooltest: `")
                    && (line.contains("did not say") || line.contains("never said")))
        })
        .map(str::trim)
        .collect();
    if !failed.is_empty() || !log.contains("nettooltest: every check passed") || code != "0x0" {
        return Err(format!("nettooltest {who} failed ({code}): {}", failed.join("; ")));
    }
    Ok(())
}
