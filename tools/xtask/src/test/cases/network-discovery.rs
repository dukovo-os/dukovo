// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Two machines on a cable find each other (ADRs 0336 to 0338), each from
//! the test image, with discotest (userspace/posix/discotest.c): QEMU's
//! stream network joins machine a's card to machine b's, and dumps what
//! goes over it. Each names itself a or b with hostname(1), puts itself at
//! 192.168.50.N and fd50::N through network(5), and starts discoverd(8),
//! whose test file leaves it off until then. a must ping a.local, and
//! b.local by IPv4 and IPv6, through resolved(8), which asks discoverd,
//! and b must ping a.local; discover(1) must list b's workstation service
//! by DNS-SD and b as a WS-Discovery device, which b's discotest answers
//! as a Windows machine does; and a ping socket must reach b with the
//! local right, not with the send right alone. The dump must hold mDNS
//! over IPv4 and IPv6, and WS-Discovery's probe.

use super::super::*;

/// What machine b runs: the same program, at the cable's other end.
const PEER_CMDLINE: &str = "lockdep -- --service-dir /system/tests/services/discovery --posix --network /system/tests/posix/discotest b";

pub const CASE: Case = Case {
    name: "network-discovery",
    about: "two machines on a cable find each other: each named with hostname(1) answers for NAME.local through discoverd's mDNS; a pings a.local and b.local by IPv4 and IPv6 and b pings a.local; discover lists b's workstation by DNS-SD and b by WS-Discovery; b is reached with the local right, not the send right alone (ADRs 0336-0338)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep -- --service-dir /system/tests/services/discovery --posix --network /system/tests/posix/discotest a",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-netdev",
        CABLE[0],
        "-device",
        "virtio-net-pci,netdev=cable,mac=52:54:00:12:34:80",
        "-object",
        CABLE_DUMP,
        PEER,
        PEER_CMDLINE,
        "-netdev",
        CABLE[1],
        "-device",
        "virtio-net-pci,netdev=cable,mac=52:54:00:12:34:81",
    ],
    script: network_discovery,
    disks: &[],
    second: None,
};

/// What discotest a says once each part has passed.
const PARTS: &[&str] = &[
    "discotest: a and b found each other's names",
    "discotest: discover listed b's workstation and b as a WS-Discovery device",
    "discotest: b was reached with the local right, not with the send right alone",
];

fn network_discovery(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect("discotest: waiting for b on port 7000")?;
    vm.boot_peer()?;
    {
        let b = vm.peer()?;
        b.expect("Boot complete.")?;
        b.expect_anywhere("discotest: connected to a")?;
    }
    let code = program_line(vm, "(/system/tests/posix/discotest) exited with code ")?;
    passed(vm, "a", &code)?;
    let log = vm.log();
    if let Some(part) = PARTS.iter().find(|part| !log.contains(*part)) {
        return Err(format!("discotest a never said \"{part}\""));
    }
    let b = vm.peer()?;
    let code = program_line(b, "(/system/tests/posix/discotest) exited with code ")?;
    passed(b, "b", &code)?;
    if !b.log().contains("discotest: answering WS-Discovery's probe urn:uuid:") {
        return Err("b never answered WS-Discovery's probe".into());
    }
    cable(&vm.dir().join(format!("{}.pcap", case.name)))
}

/// Checks that the discotest of machine `who` passed, by its log and its
/// exit code.
fn passed(vm: &Vm, who: &str, code: &str) -> Result<(), String> {
    let log = vm.log();
    let failed: Vec<&str> =
        log.lines().filter(|line| line.contains("discotest: FAILED")).map(str::trim).collect();
    if !failed.is_empty() || !log.contains("discotest: every check passed") || code != "0x0" {
        return Err(format!("discotest {who} failed ({code}): {}", failed.join("; ")));
    }
    Ok(())
}

/// Checks the dump of the cable for mDNS over IPv4 (224.0.0.251) and IPv6
/// (ff02::fb), on port 5353, and WS-Discovery's probe to 239.255.255.250
/// port 3702, each in frames to the group's multicast MAC address.
fn cable(path: &Path) -> Result<(), String> {
    let dump = std::fs::read(path).map_err(|err| format!("reading {}: {err}", path.display()))?;
    // A pcap file: a 24-byte header, then each frame after a 16-byte one
    // whose third word is its length.
    let (mut at, mut mdns4, mut mdns6, mut wsd) = (24, 0, 0, 0);
    while at + 16 <= dump.len() {
        let length = u32::from_le_bytes([dump[at + 8], dump[at + 9], dump[at + 10], dump[at + 11]]);
        let frame = &dump[at + 16..(at + 16 + length as usize).min(dump.len())];
        at += 16 + length as usize;
        if frame.len() < 14 {
            continue;
        }
        let kind = u16::from_be_bytes([frame[12], frame[13]]);
        let ip = &frame[14..];
        // UDP's destination port, after an IPv4 header of its length or
        // IPv6's 40 bytes.
        let port = |header: usize| {
            ip.get(header + 2..header + 4).map(|p| u16::from_be_bytes([p[0], p[1]]))
        };
        match kind {
            0x0800 if ip.len() >= 20 && ip[9] == 17 => {
                let header = usize::from(ip[0] & 0x0f) * 4;
                match (&ip[16..20], port(header)) {
                    ([224, 0, 0, 251], Some(5353)) if frame[..3] == [0x01, 0x00, 0x5e] => {
                        mdns4 += 1;
                    }
                    ([239, 255, 255, 250], Some(3702)) => wsd += 1,
                    _ => {}
                }
            }
            0x86dd if ip.len() >= 40 && ip[6] == 17 => {
                let group = [0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xfb];
                if ip[24..40] == group && port(40) == Some(5353) && frame[..2] == [0x33, 0x33] {
                    mdns6 += 1;
                }
            }
            _ => {}
        }
    }
    if mdns4 == 0 || mdns6 == 0 || wsd == 0 {
        return Err(format!(
            "the cable carried {mdns4} mDNS frames over IPv4, {mdns6} over IPv6 and {wsd} \
             WS-Discovery probes: some of each were wanted"
        ));
    }
    Ok(())
}
