// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The ICMP errors that report on what sockets sent, and the tools that
//! read them (ADR 0345), against routers that tracetest
//! (userspace/posix/tracetest.c) plays on a tunnel, trace0: hop 1 says
//! "time exceeded" quoting the whole packet, hop 2 nothing, hop 3 "time
//! exceeded" quoting 8 bytes past the IP header, as RFC 792's routers
//! do, and the far machine, 10.77.9.9 and fd77::99, answers pings and
//! says "port unreachable" to UDP; a name server there names them. UDP
//! and ping sockets must find each error in IP_RECVERR's queue, by IPv4
//! and IPv6, a connected socket must fail with ECONNREFUSED and not hear
//! a soft error, and datagrams sent together must keep their own hop
//! limits. traceroute and traceroute6 must find the hops by UDP and
//! ICMP, numbers and names; mtr must report each hop, with names and the
//! AS numbers it asks DNS for, by IPv4 and IPv6; and ping must say which
//! router's hop limit ran out, as iputils does.

use super::super::*;

pub const CASE: Case = Case {
    name: "network-trace",
    about: "routers played on a tunnel: sockets read their ICMP errors with IP_RECVERR as on Linux, and traceroute, traceroute6, mtr and ping find and name the hops by IPv4 and IPv6 (ADR 0345)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep -- --posix --network /system/tests/posix/tracetest",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: network_trace,
    disks: &[],
    second: None,
};

/// What tracetest says once each part has passed.
const PARTS: &[&str] = &[
    "tracetest: sockets heard the routers' errors as on Linux",
    "tracetest: traceroute found the hops by UDP and ICMP, IPv4 and IPv6, and their names",
    "tracetest: mtr measured each hop, by IPv4 and IPv6, with names and AS numbers",
    "tracetest: ping told the routers' errors as iputils does",
];

fn network_trace(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("Net: trace0 is a tunnel")?;
    vm.expect_anywhere("tracetest: trace0 is up, with its router")?;
    let code = program_line(vm, "(/system/tests/posix/tracetest) exited with code ")?;
    let log = vm.log();
    let failed: Vec<&str> =
        log.lines().filter(|line| line.contains("tracetest: FAILED")).map(str::trim).collect();
    if !failed.is_empty() || !log.contains("tracetest: every check passed") || code != "0x0" {
        return Err(format!("tracetest failed ({code}): {}", failed.join("; ")));
    }
    if let Some(part) = PARTS.iter().find(|part| !log.contains(*part)) {
        return Err(format!("tracetest never said \"{part}\""));
    }
    Ok(())
}
