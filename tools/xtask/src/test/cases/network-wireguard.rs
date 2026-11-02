// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! WireGuard between two machines on a cable (ADRs 0339 and 0340), each
//! from the test image with wgd(8), whose files wgtest
//! (userspace/posix/wgtest.c) writes and has it read again. Machine a,
//! at 192.168.50.1, listens on port 51820 and learns b's endpoint from
//! b's packets; b, at 192.168.50.2, knows a's, with a preshared key and a
//! keepalive. wg0 must be a tunnel, as the kernel, ip(8) and wg(8) say;
//! b must reach a's page through it by IPv4 and IPv6, and each ping the
//! other; a reload of wgd must keep the session, its handshake unchanged;
//! and a must refuse what b sends from an address b's AllowedIPs leave
//! out, until they hold it again. The kernel must log both tunnels made,
//! and the cable's dump must carry WireGuard's UDP datagrams on port
//! 51820 and never the page's words, which went sealed inside them.

use super::super::*;

/// What machine b runs: the same program, on its own card at the
/// cable's other end.
const PEER_CMDLINE: &str = "lockdep -- --service-dir /system/tests/services/wireguard --posix --network /system/tests/posix/wgtest b";

pub const CASE: Case = Case {
    name: "network-wireguard",
    about: "two machines on a cable with wgd: wg0 is a tunnel as the kernel, ip and wg say; b reaches a's page through it by IPv4 and IPv6 and they ping; a reload keeps the session; a refuses b's packets from an address its AllowedIPs leave out; the cable carries only WireGuard's sealed datagrams (ADRs 0339-0340)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep -- --service-dir /system/tests/services/wireguard --posix --network /system/tests/posix/wgtest a",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-netdev",
        CABLE[0],
        "-device",
        "virtio-net-pci,netdev=cable,mac=52:54:00:12:34:90",
        "-object",
        CABLE_DUMP,
        PEER,
        PEER_CMDLINE,
        "-netdev",
        CABLE[1],
        "-device",
        "virtio-net-pci,netdev=cable,mac=52:54:00:12:34:91",
    ],
    script: network_wireguard,
    disks: &[],
    second: None,
};

/// What wgtest a says once each part has passed.
const PARTS: &[&str] = &[
    "wgtest: wg0 is a tunnel, as ip, wg and the kernel say",
    "wgtest: b reached a through the tunnel by IPv4 and IPv6",
    "wgtest: a reload of wgd kept the session",
    "wgtest: a refused b's packets from an address its AllowedIPs left out",
];

/// The words of a's page, which must never cross the cable in the clear.
const PAGE: &[u8] = b"wgtest: a page that went through the tunnel";

fn network_wireguard(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("Net: wg0 is a tunnel")?;
    vm.expect_anywhere("wgd: wg0 is up, on UDP port 51820")?;
    vm.expect("wgtest: waiting for b on port 7000")?;
    vm.boot_peer()?;
    {
        let b = vm.peer()?;
        b.expect("Boot complete.")?;
        b.expect_anywhere("Net: wg0 is a tunnel")?;
        b.expect_anywhere("wgtest: connected to a through the tunnel")?;
    }
    let code = program_line(vm, "(/system/tests/posix/wgtest) exited with code ")?;
    passed(vm, "a", &code)?;
    let log = vm.log();
    if let Some(part) = PARTS.iter().find(|part| !log.contains(*part)) {
        return Err(format!("wgtest a never said \"{part}\""));
    }
    let b = vm.peer()?;
    let code = program_line(b, "(/system/tests/posix/wgtest) exited with code ")?;
    passed(b, "b", &code)?;
    cable(&vm.dir().join(format!("{}.pcap", case.name)))
}

/// Checks that the wgtest of machine `who` passed, by its log and its
/// exit code.
fn passed(vm: &Vm, who: &str, code: &str) -> Result<(), String> {
    let log = vm.log();
    let failed: Vec<&str> =
        log.lines().filter(|line| line.contains("wgtest: FAILED")).map(str::trim).collect();
    if !failed.is_empty() || !log.contains("wgtest: every check passed") || code != "0x0" {
        return Err(format!("wgtest {who} failed ({code}): {}", failed.join("; ")));
    }
    Ok(())
}

/// Checks the dump of the cable for WireGuard's datagrams, from and to
/// port 51820 over IPv4, of each of its four kinds' sizes, and that the
/// page's words are nowhere in it.
fn cable(path: &Path) -> Result<(), String> {
    let dump = std::fs::read(path).map_err(|err| format!("reading {}: {err}", path.display()))?;
    if dump.windows(PAGE.len()).any(|window| window == PAGE) {
        return Err("the cable carried a's page in the clear".into());
    }
    // A pcap file: a 24-byte header, then each frame after a 16-byte one
    // whose third word is its length.
    let (mut at, mut handshakes, mut data) = (24, 0, 0);
    while at + 16 <= dump.len() {
        let length = u32::from_le_bytes([dump[at + 8], dump[at + 9], dump[at + 10], dump[at + 11]]);
        let frame = &dump[at + 16..(at + 16 + length as usize).min(dump.len())];
        at += 16 + length as usize;
        if frame.len() < 14 + 20 + 8 + 4 || frame[12..14] != [0x08, 0x00] || frame[14 + 9] != 17 {
            continue;
        }
        let header = 14 + usize::from(frame[14] & 0x0f) * 4;
        let (Some(udp), Some(kind)) = (frame.get(header..header + 8), frame.get(header + 8)) else {
            continue;
        };
        let ports = [u16::from_be_bytes([udp[0], udp[1]]), u16::from_be_bytes([udp[2], udp[3]])];
        if !ports.contains(&51820) {
            continue;
        }
        // WireGuard's message types: 1 and 2 the handshake, 4 data.
        match kind {
            1 | 2 => handshakes += 1,
            4 => data += 1,
            _ => {}
        }
    }
    if handshakes < 2 || data == 0 {
        return Err(format!(
            "the cable carried {handshakes} WireGuard handshake messages and {data} data ones on \
             port 51820: a handshake each way and data were wanted"
        ));
    }
    Ok(())
}
