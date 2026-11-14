// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! authd blocks an address that guesses passwords, and the firewall drops
//! what it starts on the sign-in ports (ADR 0479), on two machines on a
//! cable, each from the test image, with blockingtest
//! (userspace/posix/blockingtest.c); the two-machine setup is
//! network-serve's: machine a's card is an e1000e at 10.9.0.1, which
//! gives machine b's virtio card 10.9.0.20 by DHCP. Machine a runs the
//! system's own services: alice's and carol's accounts, the second an
//! administrator's, passwords turned on in sshd_config and sshd on, and a
//! firewall rule that lets b reach the control port 7000, which is no
//! sign-in port. Machine b guesses passwords at a's sshd, with ssh and an
//! askpass program, as an attacker would, and drives nothing else.
//!
//! Five wrong passwords from b, for five names, must block b's address:
//! *firewall blocked* lists it with the reason, authd's audit line says so
//! and the kernel on a logs the connections it dropped to port 22 as
//! blocked. b's connections to port 22 must then time out, while those to
//! port 7000 go through and the control connection b has open goes on.
//! *firewall unblock* must let b in at once, and a second must say that it
//! was not blocked. *firewall never-block* must put 10.9.0.20 in
//! blocking.toml, so that six more wrong passwords block nothing, and
//! *never-block remove* must take it out again. A block of b made then must
//! outlast a restart of authd, in authd's list, its state file and the
//! kernel. With carol signed in from b, five more wrong passwords from it
//! block nothing, and carol's notice at her next sign-in tells of the two
//! blocks of b. The power button ends it.

use super::super::*;

/// What machine b runs: the same program, on its virtio card at the
/// cable's other end, with only netd to ask a for its address.
const PEER_CMDLINE: &str = "lockdep -- --service-dir /system/tests/services/blocking --posix --network /system/tests/posix/blockingtest b";

pub const CASE: Case = Case {
    name: "blocking",
    about: "two machines on a cable: b's five wrong passwords over SSH, for five names, block its address, which firewall blocked lists with the reason; its connections to port 22 then time out while those to port 7000 go through; firewall unblock lets it in at once; with its address in never six more wrong passwords block nothing; a block outlasts a restart of authd; with an administrator signed in from b, five more block nothing; the kernel logs what it dropped as blocked, and authd the blocks, unblocks and the change of settings (ADR 0479)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // Every service, as on a machine, with the capabilities of the
    // firewall and of the names the test program uses.
    cmdline: "lockdep -- --services --posix --network --firewall --names /system/tests/posix/blockingtest a",
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
    script: blocking,
    disks: &[],
    second: None,
};

/// What blockingtest a says once each part has passed.
const PARTS: &[&str] = &[
    "blockingtest: five wrong passwords from b, for five names, blocked its address, and firewall blocked says why",
    "blockingtest: b's connections to port 22 timed out while those to port 7000 went through",
    "blockingtest: firewall unblock let b in at once, and a second one said it was not blocked",
    "blockingtest: with b's address in never, six more wrong passwords blocked nothing",
    "blockingtest: a block outlasted a restart of authd, in its list, its state file and the kernel",
    "blockingtest: five wrong passwords from b blocked nothing while an administrator was signed in from it",
    "blockingtest: the administrator's notice told of the blocks",
];

/// What authd on a logs of what it did, once the parts have passed, as the
/// audit log's lines show on the serial line, and how many times each: the
/// two blocks of b, with the reason's start; the two unblocks; the network
/// put in never and taken out; and the settings read again with b's
/// address in never.
const AUDIT: &[(&str, usize)] = &[
    ("authd: blocked 10.9.0.20 for 10 minutes: 5 wrong passwords over SSH in ", 2),
    ("authd: unblocked 10.9.0.20, for process ", 2),
    ("authd: 10.9.0.20 is never blocked, for process ", 1),
    ("authd: 10.9.0.20 may be blocked again, for process ", 1),
    (
        "authd: blocking addresses that guess passwords: on: 5 tries within 10 minutes block for \
         10 minutes, doubling to 1 day, never 10.9.0.20",
        1,
    ),
];

fn blocking(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("Net: net0 is an Intel 82574L network card")?;
    // a is set up before b boots, so that b's DHCP request is answered.
    vm.expect("blockingtest: waiting for b on port 7000")?;
    vm.boot_peer()?;
    {
        let b = vm.peer()?;
        b.expect("Boot complete.")?;
        b.expect_anywhere("blockingtest: connected to a")?;
    }
    let code = program_line(vm, "(/system/tests/posix/blockingtest) exited with code ")?;
    passed(vm, "a", &code)?;
    let log = vm.log();
    if let Some(part) = PARTS.iter().find(|part| !log.contains(*part)) {
        return Err(format!("machine a never said \"{part}\""));
    }
    for (line, times) in AUDIT {
        let found = log.matches(line).count();
        if found < *times {
            return Err(format!("authd on a logged \"{line}\" {found} times, not {times}"));
        }
    }
    dropped_as_blocked(&log)?;
    let b = vm.peer()?;
    let code = program_line(b, "(/system/tests/posix/blockingtest) exited with code ")?;
    passed(b, "b", &code)
}

/// Checks that the kernel on a logged a connection of b's dropped to port
/// 22 as blocked, and none to port 7000, which no block closes.
fn dropped_as_blocked(log: &str) -> Result<(), String> {
    let drops = |port: u16| {
        log.lines().filter(move |line| {
            line.contains("Firewall: dropped a TCP connection from 10.9.0.20:")
                && line.contains(&format!(" to port {port} on net0: "))
        })
    };
    if !drops(22).any(|line| line.trim_end().ends_with("the address is blocked")) {
        return Err(
            "the kernel on a never logged a connection to port 22 dropped as blocked".into()
        );
    }
    if let Some(line) = drops(7000).next() {
        return Err(format!(
            "the kernel on a dropped a connection to the control port: {}",
            line.trim()
        ));
    }
    Ok(())
}

/// Checks that the blockingtest of machine `who` passed, by its log and
/// its exit code. What failed comes with what blockingtest said about it,
/// such as what a command wrote, as the log's tail may not hold it.
fn passed(vm: &Vm, who: &str, code: &str) -> Result<(), String> {
    let log = vm.log();
    let failed: Vec<&str> =
        log.lines().filter(|line| line.contains("blockingtest: FAILED")).map(str::trim).collect();
    if !failed.is_empty() || !log.contains("blockingtest: every check passed") || code != "0x0" {
        return Err(format!("blockingtest {who} failed ({code}): {}", failed.join("; ")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drop_to_port_22_as_blocked_is_told_from_a_drop_to_the_control_port() {
        let dropped = |port: u16, why: &str| {
            format!(
                "[ 90.1] Firewall: dropped a TCP connection from 10.9.0.20:51334 to port {port} \
                 on net0: {why}\n"
            )
        };
        let blocked = dropped(22, "the address is blocked");
        assert_eq!(dropped_as_blocked(&blocked), Ok(()));
        // A drop to port 22 by a rule is not a block's.
        assert!(
            dropped_as_blocked(&dropped(22, "process 41 listens for this machine alone")).is_err()
        );
        // Another address's is not b's.
        assert!(dropped_as_blocked(&blocked.replace("10.9.0.20", "10.9.0.21")).is_err());
        // A drop to the control port is a block gone too far.
        let both = format!("{blocked}{}", dropped(7000, "the address is blocked"));
        assert!(dropped_as_blocked(&both).is_err());
        assert!(dropped_as_blocked("nothing here").is_err());
    }
}
