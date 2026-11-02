// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Two machines on a cable (ADRs 0331 to 0335), each from the test image:
//! QEMU's stream network joins machine a's second card to machine b's
//! card, and dumps what goes over it. Machine a (userspace/posix/pairtest.c)
//! makes the bridge br0 of its card on QEMU's user network and the cable,
//! and VLAN 7 on the bridge at 192.168.7.1 and fd07::1, through ip(8) and
//! network(5); the kernel must say so. Machine b boots once a's bridge is
//! up, and must get its IPv4 address by DHCP, its IPv6 router and stable
//! and temporary addresses by SLAAC and its DNS server by DHCPv6, all
//! from QEMU's network through a's bridge, and VLAN 7 at 192.168.7.2 and
//! fd07::2. b's curl must fetch a's page over VLAN 7 by IPv6 and IPv4,
//! and over the bridge at a's DHCP and SLAAC addresses. Then the
//! firewall, probed from b: a program a runs as a person would, without
//! the inbound right, must be reachable from a alone, which firewall
//! status must say along with the command that allows it, until `firewall
//! allow` names its program, and again once that rule is removed; a deny
//! rule must keep fd07::2 from a page that 192.168.7.2 still reaches; and
//! port 22 must be open only while the port22 service, sshd's stand-in,
//! runs, and port 23 never. The kernel must log what it dropped, and the
//! dump must hold frames tagged for VLAN 7, IPv4 and IPv6.

use super::super::*;

/// What machine b runs: the same program, on its own card at the
/// cable's other end.
const PEER_CMDLINE: &str =
    "lockdep -- --service-dir /system/tests/services/pair --posix --network /system/tests/posix/pairtest b";

pub const CASE: Case = Case {
    name: "network-pair",
    about: "two machines on a cable: a bridges its network and the cable with VLAN 7 on the bridge; b gets DHCP, SLAAC and DHCPv6 through the bridge and fetches a's page by IPv6 and IPv4 over both; the firewall keeps a person's program and port 22 from b until a rule or the service opens them (ADRs 0331-0335)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep -- --service-dir /system/tests/services/pair --posix --network --firewall /system/tests/posix/pairtest a",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-netdev",
        "user,id=n0",
        "-device",
        "virtio-net-pci,netdev=n0,mac=52:54:00:12:34:70",
        "-netdev",
        CABLE[0],
        "-device",
        "virtio-net-pci,netdev=cable,mac=52:54:00:12:34:71",
        "-object",
        CABLE_DUMP,
        PEER,
        PEER_CMDLINE,
        "-netdev",
        CABLE[1],
        "-device",
        "virtio-net-pci,netdev=cable,mac=52:54:00:12:34:72",
    ],
    script: network_pair,
    disks: &[],
    second: None,
};

/// What the kernel says of a's links, in its log.
const LINKS: &[&str] = &[
    "Net: br0 is a bridge",
    "Net: net0 is a port of br0",
    "Net: net1 is a port of br0",
    "Net: br0.7 is VLAN 7 on br0",
];

/// What b says of the addresses it got through a's bridge.
const THROUGH_THE_BRIDGE: &[&str] = &[
    "netd: net0 has 10.0.2.",
    "Net: net0 has the IPv6 router fe80::2",
    " (stable)",
    " (temporary)",
    "netd: net0 has DNS fec0::3 by DHCPv6",
    "Net: net0.7 is VLAN 7 on net0",
];

/// What pairtest a says once each part has passed.
const PARTS: &[&str] = &[
    "pairtest: b fetched a's page over VLAN 7 and the bridge, by IPv6 and IPv4",
    "pairtest: a person's program was for this machine alone until a rule allowed it",
    "pairtest: a rule denied port 8080 to fd07::2 alone",
    "pairtest: port 22 was open only while its service ran",
];

fn network_pair(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    for line in LINKS {
        vm.expect_anywhere(line)?;
    }
    // b boots once a's bridge is up, so that its first DHCP request and
    // router solicitation go through it.
    vm.expect("pairtest: waiting for b on port 7000")?;
    vm.boot_peer()?;
    {
        let b = vm.peer()?;
        b.expect("Boot complete.")?;
        for line in THROUGH_THE_BRIDGE {
            b.expect_anywhere(line)?;
        }
        b.expect_anywhere("pairtest: connected to a")?;
    }
    for part in PARTS {
        vm.expect_ahead(part)?;
    }
    let code = program_line(vm, "(/system/tests/posix/pairtest) exited with code ")?;
    passed(vm, "a", &code)?;
    let b = vm.peer()?;
    let code = program_line(b, "(/system/tests/posix/pairtest) exited with code ")?;
    passed(b, "b", &code)?;
    // What a dropped, it logged.
    let log = vm.log();
    for port in ["8081", "8080", "22", "23"] {
        let dropped = format!("to port {port} on br0.7");
        if !log.lines().any(|line| {
            line.contains("Firewall: dropped a TCP connection from [fd07::2]:")
                && line.contains(&dropped)
        }) {
            return Err(format!("the kernel never logged a TCP connection dropped {dropped}"));
        }
    }
    let alone = "to port 8081 on br0.7: process ";
    if !log.lines().any(|line| line.contains(alone) && line.contains("for this machine alone")) {
        return Err("the kernel never said whose program listened on 8081 for a alone".into());
    }
    cable(&vm.dir().join(format!("{}.pcap", case.name)))
}

/// Checks that the pairtest of machine `who` passed, by its log and its
/// exit code.
fn passed(vm: &Vm, who: &str, code: &str) -> Result<(), String> {
    let log = vm.log();
    let failed: Vec<&str> =
        log.lines().filter(|line| line.contains("pairtest: FAILED")).map(str::trim).collect();
    if !failed.is_empty() || !log.contains("pairtest: every check passed") || code != "0x0" {
        return Err(format!("pairtest {who} failed ({code}): {}", failed.join("; ")));
    }
    Ok(())
}

/// Checks the dump of the cable for frames tagged for VLAN 7 that carry
/// IPv4 and IPv6, and untagged ones the bridge passed on.
fn cable(path: &Path) -> Result<(), String> {
    let dump = std::fs::read(path).map_err(|err| format!("reading {}: {err}", path.display()))?;
    // A pcap file: a 24-byte header, then each frame after a 16-byte one
    // whose third word is its length.
    let (mut at, mut tagged4, mut tagged6, mut untagged) = (24, 0, 0, 0);
    while at + 16 <= dump.len() {
        let length = u32::from_le_bytes([dump[at + 8], dump[at + 9], dump[at + 10], dump[at + 11]]);
        let frame = &dump[at + 16..(at + 16 + length as usize).min(dump.len())];
        at += 16 + length as usize;
        if frame.len() < 18 {
            continue;
        }
        let kind = u16::from_be_bytes([frame[12], frame[13]]);
        let vid = u16::from_be_bytes([frame[14], frame[15]]) & 0x0fff;
        let inner = u16::from_be_bytes([frame[16], frame[17]]);
        match (kind, vid, inner) {
            (0x8100, 7, 0x0800) => tagged4 += 1,
            (0x8100, 7, 0x86dd) => tagged6 += 1,
            (0x0800 | 0x86dd, ..) => untagged += 1,
            _ => {}
        }
    }
    if tagged4 == 0 || tagged6 == 0 || untagged == 0 {
        return Err(format!(
            "the cable carried {tagged4} IPv4 and {tagged6} IPv6 frames tagged for VLAN 7, and \
             {untagged} untagged ones: some of each were wanted"
        ));
    }
    Ok(())
}
