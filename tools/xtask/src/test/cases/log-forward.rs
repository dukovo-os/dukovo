// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The log sent to syslog servers (ADR 0482), on two machines on a cable,
//! each from the test image, with logforwardtest
//! (userspace/posix/logforwardtest.c): machine a's card is an e1000e and
//! b's a virtio card. Machine b runs logd(8) and logforward(8) and sends
//! its log to four destinations on a. a runs an openssl s_server on port
//! 6514 with a certificate for a.test that a certificate authority of its
//! own signed, which b's settings name as the authority to trust; a second
//! on port 6515 with a self-signed certificate for a.test, which nothing
//! vouches for; a TCP server on port 514; and a UDP one. The record that
//! logger writes on b must reach the TLS, TCP and UDP servers as an RFC
//! 5424 message, with its length in front over TLS and TCP; the server of
//! the certificate b does not trust must be refused, with the reason in
//! the output of logforward status, and get nothing. With the first TLS
//! server stopped, the records logged meanwhile must arrive in order once
//! it is back. After a restart of the service, no record that was
//! delivered before must arrive again, and a new one must. logforward
//! test must send a test record over TLS 1.3 and say why it cannot reach
//! the server b does not trust.

use super::super::*;

/// What machine b runs: the same program, on its virtio card at the
/// cable's other end.
const PEER_CMDLINE: &str = "lockdep -- --service-dir /system/tests/services/forward --posix --network /system/tests/posix/logforwardtest b";

pub const CASE: Case = Case {
    name: "log-forward",
    about: "two machines on a cable: logforward on b sends the log to a's syslog servers over TLS to one whose certificate b trusts, TCP and UDP, and is refused by a TLS server whose certificate nothing vouches for; records logged while a server is away arrive in order when it is back, and a restart sends nothing again; logforward test sends a test record over TLS (ADR 0482)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep -- --service-dir /system/tests/services/forward --posix --network /system/tests/posix/logforwardtest a",
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
    script: log_forward,
    disks: &[],
    second: None,
};

/// What logforwardtest a says once each part has passed.
const PARTS: &[&str] = &[
    "logforwardtest: the TLS, TCP and UDP servers got the record, as RFC 5424 has it, with its length in front over TLS and TCP",
    "logforwardtest: the server whose certificate b does not trust was refused and got nothing",
    "logforwardtest: records logged while the TLS server was away arrived in order when it was back",
    "logforwardtest: after logforward restarted, nothing delivered was sent again, and what came after was",
    "logforwardtest: logforward test sent a test record over TLS and said why it cannot reach the server b does not trust",
];

fn log_forward(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("Net: net0 is an Intel 82574L network card")?;
    vm.expect("logforwardtest: waiting for b on port 7000")?;
    vm.boot_peer()?;
    {
        let b = vm.peer()?;
        b.expect("Boot complete.")?;
        b.expect_anywhere("logforwardtest: connected to a")?;
    }
    let code = program_line(vm, "(/system/tests/posix/logforwardtest) exited with code ")?;
    passed(vm, "a", &code)?;
    let log = vm.log();
    if let Some(part) = PARTS.iter().find(|part| !log.contains(*part)) {
        return Err(format!("logforwardtest a never said \"{part}\""));
    }
    let b = vm.peer()?;
    let code = program_line(b, "(/system/tests/posix/logforwardtest) exited with code ")?;
    passed(b, "b", &code)
}

/// Checks that the logforwardtest of machine `who` passed, by its log and
/// its exit code. What failed comes with what logforwardtest said about
/// it, such as how a command ended, as the log's tail may not hold it.
fn passed(vm: &Vm, who: &str, code: &str) -> Result<(), String> {
    let log = vm.log();
    let failed: Vec<&str> = log
        .lines()
        .filter(|line| {
            line.contains("logforwardtest: FAILED")
                || (line.contains("logforwardtest: `")
                    && (line.contains("never said") || line.contains("never moved")))
        })
        .map(str::trim)
        .collect();
    if !failed.is_empty() || !log.contains("logforwardtest: every check passed") || code != "0x0" {
        return Err(format!("logforwardtest {who} failed ({code}): {}", failed.join("; ")));
    }
    Ok(())
}
