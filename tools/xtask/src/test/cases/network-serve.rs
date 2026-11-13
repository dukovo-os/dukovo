// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A machine that gives its network addresses and the time (ADR 0486), on
//! two machines on a cable, each from the test image, with networkservetest
//! (userspace/posix/networkservetest.c); the two-machine setup is
//! log-forward's: machine a's card is an e1000e and b's a virtio card.
//! Machine a gives 10.9.0.100 to 10.9.0.150 by DHCP for a minute, and b's
//! MAC address 10.9.0.20 as b, with a router, two DNS servers and a domain
//! that are set, its own address for a time server as its timed serves the
//! time, and the metered flag. Machine b asks for an address: it must get
//! 10.9.0.20 with all of that, a's netd leases must list it by name, its
//! lease must be renewed at the half of it and end later, and netd on b must
//! say the connection is metered to updated(8). a's timed serves NTS and
//! plain NTP with a certificate for a.test from an authority of its own,
//! from a clock nothing set; b's clock starts an hour ahead, and b's timed,
//! which trusts the authority and finds a.test in its hosts file, must take
//! the time from a over NTS, which the kernel logs as a step of an hour
//! back, and leave b's clock within a second of a's. A plain NTP query from
//! b must get an answer at stratum 10 from a, and none once a serves NTS
//! alone. With netd on b stopped, a client of another MAC address must be
//! offered the lowest address of the pool with the options that are set,
//! get it, and give it back, which a's netd logs; b's own MAC address must
//! be offered its reservation.

use super::super::*;

/// What machine b runs: the same program, on its virtio card at the
/// cable's other end, with a clock that starts an hour ahead.
const PEER_CMDLINE: &str = "lockdep -- --service-dir /system/tests/services/serve --posix --network /system/tests/posix/networkservetest b";

pub const CASE: Case = Case {
    name: "network-serve",
    about: "two machines on a cable: a's netd gives b the address reserved for it with the router, DNS servers, domain, time server and metered flag that are set, lists it, renews it and takes a second client's lease and release; b, whose clock starts an hour ahead, takes the time from a's timed over NTS; a plain NTP query gets an answer while a serves it and none once it serves NTS alone (ADR 0486)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep -- --service-dir /system/tests/services/serve --posix --network /system/tests/posix/networkservetest a",
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
        "-rtc",
        RTC_AN_HOUR_AHEAD,
    ],
    script: network_serve,
    disks: &[],
    second: None,
};

/// What networkservetest a says once each part has passed.
const PARTS: &[&str] = &[
    "networkservetest: b got 10.9.0.20 with the router, DNS servers and domain that are set, and a lists it by name",
    "networkservetest: b's netd says the connection is metered",
    "networkservetest: b started an hour ahead, and timed took the time from a over NTS and stepped the clock to within a second of a's",
    "networkservetest: b renewed its lease at the half of it, and the end moved on",
    "networkservetest: a plain NTP query got an answer from a stratum 10 server, and none once it served NTS alone",
    "networkservetest: a second client was offered the lowest address of the pool with what is set, got it, and gave it back",
];

/// What netd on a logs of what it gave, once the parts have passed: the
/// server's start, the leases and a release.
const A_LOG: &[&str] = &[
    "netd: net0: giving addresses 10.9.0.100-10.9.0.150 by DHCP, for 1 minute each",
    "netd: net0: gave 10.9.0.20 to 52:54:00:12:34:a1 (b) for 1 minute",
    "netd: net0: gave 10.9.0.100 to 52:54:00:99:00:01 for 1 minute",
    "netd: net0: 52:54:00:99:00:01 released 10.9.0.100",
];

/// What machine b logs: its lease with what came with it, that the
/// connection is metered, and its lease kept at a renewal.
const B_LOG: &[&str] = &[
    "netd: net0 has 10.9.0.20/24 from 10.9.0.1 for 1 minute, router 10.9.0.2, DNS 10.9.0.53 10.9.0.54",
    "netd: net0 is metered: updates wait for another connection",
    "netd: net0 keeps 10.9.0.20/24 from 10.9.0.1 for 1 minute",
];

fn network_serve(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("Net: net0 is an Intel 82574L network card")?;
    vm.expect("networkservetest: waiting for b on port 7000")?;
    // The server must answer before b asks.
    vm.expect_anywhere(A_LOG[0])?;
    vm.boot_peer()?;
    {
        let b = vm.peer()?;
        b.expect("Boot complete.")?;
        b.expect_anywhere("networkservetest: connected to a")?;
    }
    let code = program_line(vm, "(/system/tests/posix/networkservetest) exited with code ")?;
    passed(vm, "a", &code)?;
    let log = vm.log();
    if let Some(part) = PARTS.iter().chain(A_LOG).find(|part| !log.contains(*part)) {
        return Err(format!("machine a never said \"{part}\""));
    }
    let b = vm.peer()?;
    let code = program_line(b, "(/system/tests/posix/networkservetest) exited with code ")?;
    passed(b, "b", &code)?;
    let log = b.log();
    if let Some(line) = B_LOG.iter().find(|line| !log.contains(*line)) {
        return Err(format!("machine b never said \"{line}\""));
    }
    stepped_an_hour(&log)
}

/// Checks that the kernel on b logged timed's step of the clock, which
/// started an hour ahead: a step back of an hour, within a minute and a
/// half, the clock being read some seconds after it was set.
fn stepped_an_hour(log: &str) -> Result<(), String> {
    let line = log
        .lines()
        .find(|line| line.contains("Time: timed set the clock to "))
        .ok_or("the kernel on b never said that timed set the clock")?;
    let ms: Option<i64> = line
        .split_once(" UTC (")
        .and_then(|(_, rest)| rest.split_once(" ms)"))
        .and_then(|(number, _)| number.parse().ok());
    match ms {
        Some(ms) if (-3_690_000..=-3_510_000).contains(&ms) => Ok(()),
        _ => Err(format!("timed did not step b's clock back an hour: {}", line.trim())),
    }
}

/// Checks that the networkservetest of machine `who` passed, by its log and
/// its exit code. What failed comes with what networkservetest said about
/// it, such as what a command wrote, as the log's tail may not hold it.
fn passed(vm: &Vm, who: &str, code: &str) -> Result<(), String> {
    let log = vm.log();
    let failed: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("networkservetest: FAILED"))
        .map(str::trim)
        .collect();
    if !failed.is_empty() || !log.contains("networkservetest: every check passed") || code != "0x0"
    {
        return Err(format!("networkservetest {who} failed ({code}): {}", failed.join("; ")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernel_s_step_of_an_hour_back_is_told_from_other_steps() {
        let step = |ms: &str| {
            format!("[ 41.2] Time: timed set the clock to 2026-11-13 10:00:00 UTC ({ms} ms)")
        };
        assert_eq!(stepped_an_hour(&step("-3598811")), Ok(()));
        assert!(stepped_an_hour(&step("-3600042")).is_ok());
        assert!(stepped_an_hour(&step("-5000")).is_err());
        assert!(stepped_an_hour(&step("+3598811")).is_err());
        assert!(stepped_an_hour("nothing here").is_err());
    }
}
