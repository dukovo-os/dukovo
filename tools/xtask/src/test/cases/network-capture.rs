// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Packet captures (ADR 0346), as capturetest
//! (userspace/posix/capturetest.c) opens them with radix-network(2)'s
//! open_capture and as tcpdump(8) reads them through libpcap, on lo, on
//! a tunnel the test carries, cap0, on a virtio card on QEMU's user
//! network, net0, and on every interface at once. The kernel's frames
//! must come whole with their headers and times, lo's behind an Ethernet
//! header of zero addresses as on Linux, cut to the snap length, the
//! filter's length or a read's buffer; classic BPF filters must pick
//! them, and programs the kernel does not run be refused; a full buffer
//! must lose frames and count them; a capture of an interface taken away
//! must read what waited, then fail with ENXIO; one without the control
//! right is refused, and one past the most there may be. tcpdump must
//! list the interfaces, read lo's datagrams and pings with the kernel's
//! filter, every interface's cooked as Linux's LINUX_SLL and LINUX_SLL2,
//! cap0's IP packets each way alone, and net0's TCP handshake; write a
//! capture file by microseconds and nanoseconds and read it back; say
//! why it may not capture; and stop at SIGINT with its counts.

use super::super::*;

pub const CASE: Case = Case {
    name: "network-capture",
    about: "packet captures (ADR 0346): the kernel's frames of lo, a tunnel, a virtio card and every interface, cut, filtered by classic BPF and counted, and tcpdump through libpcap reading them, cooked, into files and back, by direction",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep ip=10.0.2.15/24 gateway=10.0.2.2 -- --posix --network /system/tests/posix/capturetest",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", ECHO_NETDEV, "-device", "virtio-net-pci,netdev=n0"],
    script: network_capture,
    disks: &[],
    second: None,
};

/// What capturetest says once each part has passed.
const PARTS: &[&str] = &[
    "capturetest: lo's frames came whole, timed, and cut to their reads",
    "capturetest: captures kept their snap lengths and refused what is out of range",
    "capturetest: filters picked and cut frames, and bad programs were refused",
    "capturetest: a full buffer lost frames, and counted them",
    "capturetest: cap0's packets were captured both ways, and every interface's at once",
    "capturetest: a capture of a tunnel taken away read what waited, then ENXIO",
    "capturetest: a capture needed the control right, and tcpdump said so",
    "capturetest: 16 captures were the most there may be",
    "capturetest: tcpdump listed the interfaces",
    "capturetest: tcpdump read lo's datagrams and pings, filtered by the kernel",
    "capturetest: tcpdump read every interface's frames, cooked as on Linux",
    "capturetest: tcpdump wrote a capture file, by microseconds and nanoseconds, and read it back",
    "capturetest: tcpdump read cap0's packets, each way alone",
    "capturetest: tcpdump read net0's frames, TCP's handshake among them",
    "capturetest: tcpdump stopped at SIGINT and said what it counted",
];

fn network_capture(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("capturetest: cap0 is up, with its router")?;
    let code = program_line(vm, "(/system/tests/posix/capturetest) exited with code ")?;
    let log = vm.log();
    let failed: Vec<&str> =
        log.lines().filter(|line| line.contains("capturetest: FAILED")).map(str::trim).collect();
    if !failed.is_empty() || !log.contains("capturetest: every check passed") || code != "0x0" {
        return Err(format!("capturetest failed ({code}): {}", failed.join("; ")));
    }
    if let Some(part) = PARTS.iter().find(|part| !log.contains(*part)) {
        return Err(format!("capturetest never said \"{part}\""));
    }
    Ok(())
}
