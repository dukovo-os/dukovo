// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! certd(8) keeps the machine's certificates, and Spark copies them for
//! the services that name them (ADR 0512). certd makes this-machine as it
//! starts, and alice, who signs in as the owner, sees it in cert list, and
//! cert check passes, with /system/state as readable as /var is. A test
//! service that names it is given both files, its own and 0400, with the
//! key's bytes certd's. Two cert renew runs a second apart, 10 seconds
//! after the service started, have it sent one SIGHUP at once and the
//! second 10 seconds later, and it then reads the newest. A certd that is
//! not what it was could do five things to its folder, and each is
//! refused with Spark's reason, with nothing of Spark's secrets reaching
//! the service: a folder where the link is, a link out of the folder, a
//! generation whose key is a link, a key of 65 KiB, and a named pipe for a
//! key. A process that is not certd asking at certd's socket is refused
//! and audited. A service file that names a certificate that does not
//! exist does not start, and says so. A server certificate whose chain
//! lacks its intermediate is refused with the issuer's name by cert add,
//! and the whole chain is added, and cert list shows it with its days; a
//! certificate that a service file names is not removed, and the message
//! names the file.

use super::super::*;

/// Alice's password, which elevate asks for.
const PASSWORD: &str = "queen of hearts";

/// The test's service files and scripts in the image.
const FILES: &str = "/system/tests/services/certificates";

/// The folder of the service files Spark reads.
const SERVICES: &str = "/system/settings/spark/services";

/// The test.
pub const CASE: Case = Case {
    name: "certificates",
    about: "certd makes this-machine as it starts, and cert list shows it to an ordinary account; a service that names it is given both files as its own, 0400, and sent one SIGHUP at once and one 10 seconds later for two renewals, and reads the newest; a folder in the link's place, a link out of the folder, a key that is a link, a key of 65 KiB and a named pipe for a key are each refused with Spark's reason and leak nothing; a process that is not certd is refused at certd's socket and audited; a service file that names a certificate that does not exist does not start; cert add refuses a chain without its intermediate, naming the issuer, adds the whole chain, and cert remove refuses a certificate a service file names (ADR 0512)",
    firmware: Firmware::Bios,
    media: Media::Usb,
    // As a normal boot: Spark starts certd with the other services.
    cmdline: "lockdep",
    cpus: 2,
    memory: "512M",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: certificates,
    disks: &[],
    second: None,
};

/// What a test service's line says as it starts and as it is sent
/// SIGHUP: the time in seconds, the mode of its key, and the first 16
/// digits of the SHA-256 of its chain and of its key.
struct Read {
    time: i64,
    mode: String,
    chain: String,
    key: String,
}

/// The last line of the log that holds `start`, as a [`Read`].
fn read_of(vm: &Vm, start: &str) -> Result<Read, String> {
    let log = vm.log();
    let line = log
        .lines()
        .rev()
        .find_map(|line| line.split_once(start).map(|(_, rest)| rest))
        .ok_or_else(|| format!("the log has no line with \"{start}\""))?;
    let words: Vec<&str> = line.split_whitespace().collect();
    let [time, mode, chain, key] = words.as_slice() else {
        return Err(format!("\"{start}{line}\" is not a time, a mode and two sums"));
    };
    Ok(Read {
        time: time.parse().map_err(|_| format!("\"{time}\" is no time in \"{start}{line}\""))?,
        mode: (*mode).into(),
        chain: (*chain).into(),
        key: (*key).into(),
    })
}

/// How many lines of the log hold `text`.
fn lines_with(vm: &Vm, text: &str) -> usize {
    vm.log().lines().filter(|line| line.contains(text)).count()
}

/// Types `command` at alice's prompt, on a cleared screen.
fn run(vm: &mut Vm, command: &str) -> Result<(), String> {
    answer_prompt(vm, "$", &format!("clear; {command}"))
}

fn certificates(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    // certd made the machine's certificate as it started, before anyone
    // signed in.
    vm.expect_anywhere("certd: made this-machine for ")?;

    // 1. cert check passes, and an ordinary account sees the certificate
    // for the machine's name and 800 days (799 once the hour it is dated
    // back is counted).
    run(
        vm,
        &format!(
            "cd; T={FILES}; echo \"state $(stat -c %a /system/state) host $(hostname)\"; cert \
             check; echo \"check $?\"; cert list"
        ),
    )?;
    vm.expect_screen(&["state 755 host ", "check 0", "NAME", "this-machine  self-signed"])?;
    let screen = vm.screen()?;
    let host = screen
        .iter()
        .find_map(|line| line.trim().strip_prefix("state 755 host "))
        .ok_or("the screen has no host")?
        .trim()
        .to_string();
    let row = screen
        .iter()
        .find(|line| line.starts_with("this-machine"))
        .ok_or("cert list shows no row of this-machine")?;
    if !row.contains(&format!("{host}, {host}.local")) {
        return Err(format!("this-machine is not for {host} and {host}.local: {row}"));
    }
    if !row.contains("(800 days)") && !row.contains("(799 days)") {
        return Err(format!("this-machine does not run for 800 days: {row}"));
    }

    // 2. A service that names it: both files, its own, and the key's
    // bytes the ones certd holds.
    run(
        vm,
        &format!(
            "elevate cp $T/certtest.toml $T/certtest2.toml {SERVICES}/ && elevate service \
             certtest start; echo \"started $?\""
        ),
    )?;
    answer_prompt(vm, ":", PASSWORD)?;
    vm.expect("certtest: start ")?;
    let started = read_of(vm, "certtest: start ")?;
    if started.mode != "400" {
        return Err(format!("the service's key has mode {}, not 400", started.mode));
    }
    run(
        vm,
        "elevate sh -c 'cd /system/state/certd/certificates/this-machine; echo certd $(sha256sum \
         < fullchain.pem | cut -c1-16) $(sha256sum < key.pem | cut -c1-16)'",
    )?;
    vm.expect_screen(&[&format!("certd {} {}", started.chain, started.key)])?;

    // 3. Two renewals a second apart, 10 seconds after the service
    // started (the sleep runs from this line, after it): one SIGHUP at
    // once and one 10 seconds after it.
    run(
        vm,
        "elevate sh -c 'sleep 11; cert renew this-machine; cert renew this-machine'; echo \
         \"renewed $?\"",
    )?;
    vm.expect("certtest: hup ")?;
    let first = read_of(vm, "certtest: hup ")?;
    vm.expect("certtest: hup ")?;
    let second = read_of(vm, "certtest: hup ")?;
    vm.expect_screen(&["renewed 0"])?;
    if !(9..=16).contains(&(second.time - first.time)) {
        return Err(format!(
            "the SIGHUPs came {} seconds apart, not about 10",
            second.time - first.time
        ));
    }
    if second.chain == started.chain || second.key == started.key {
        return Err("the service read the old certificate after its last SIGHUP".into());
    }
    run(vm, "cert show this-machine --pem | sha256sum | cut -c1-16")?;
    vm.expect_screen(&[&second.chain])?;
    let hups = lines_with(vm, "certtest: hup ");
    if hups != 2 {
        return Err(format!("the service was sent {hups} SIGHUPs for two renewals, not 2"));
    }

    // 4. What a certd that is not what it was could do to its folder:
    // each is refused when Spark starts the service, with its reason, and
    // restored before the next.
    let link = "could not start certtest: certd's this-machine is not a link to one of its \
                generations";
    let file = "could not start certtest: certd's this-machine/key.pem is not a regular file of \
                64 KiB or less";
    for (attack, reason) in
        [("folder", link), ("outside", link), ("link", file), ("big", file), ("fifo", file)]
    {
        run(vm, &format!("elevate sh $T/attack.sh {attack}; echo \"{attack} $?\""))?;
        vm.expect_screen(&[&format!("service: {reason}"), &format!("attack {attack}: restart ended with 1")])?;
        vm.expect(&format!("spark: {reason}"))?;
        run(vm, "elevate sh $T/attack.sh restore")?;
        vm.expect("spark: started certtest (/system/commands/nukesh) as process ")?;
        vm.expect_screen(&["attack restore: restart ended with 0"])?;
    }
    if vm.log().contains("LEAK-MARKER") {
        return Err("what a link led to in Spark's secrets reached the log".into());
    }

    // 5. A process that is not certd, at certd's socket.
    run(
        vm,
        "elevate sh -c 'echo certificate this-machine | nc -U -w 2 \
         /system/live/spark/certd/socket'; echo \"asked $?\"",
    )?;
    vm.expect("refused `certificate this-machine")?;
    vm.expect_screen(&["error: only certd asks through this socket"])?;

    // 6. A certificate that does not exist.
    run(vm, "elevate service certtest2 start; echo \"certtest2 $?\"")?;
    vm.expect("could not start certtest2: its certificate added.example does not exist")?;
    vm.expect_screen(&["its certificate added.example does not exist"])?;

    // 7. A chain made the way authorities make them: a server's
    // certificate without its intermediate is refused, naming the
    // issuer; with it, and the authority's root among the roots the
    // machine trusts (a file of its own in place of the link to the
    // system's, as dukovo-certificates(7) says), it is added.
    run(vm, "sh $T/chain.sh ~/chain")?;
    vm.expect_screen(&["chain made"])?;
    let add = "elevate cert add added.example --key /users/alice/chain/leaf.key --chain";
    run(vm, &format!("{add} /users/alice/chain/leaf.pem; echo \"add $?\""))?;
    vm.expect_screen(&["the chain stops at CN=Test-Intermediate", "add 1"])?;
    run(
        vm,
        &format!(
            "elevate sh -c 'cd /system/settings/ssl/certs && cat \
             /system/resources/certificates/ca-certificates.crt /users/alice/chain/root.pem > \
             new.crt && mv new.crt ca-certificates.crt'; {add} /users/alice/chain/chain.pem; echo \
             \"add $?\"; cert list"
        ),
    )?;
    vm.expect_screen(&["added.example added: added.example, runs out ", "add 0"])?;
    let screen = vm.screen()?;
    let row = screen
        .iter()
        .find(|line| line.starts_with("added.example"))
        .ok_or("cert list shows no row of added.example")?;
    if !row.contains("added") || !(row.contains("(29 days)") || row.contains("(30 days)")) {
        return Err(format!("added.example is not shown as added, with its days: {row}"));
    }

    // A service file that names it keeps it.
    run(
        vm,
        "elevate service certtest2 start; echo \"certtest2 $?\"; elevate cert remove \
         added.example; echo \"remove $?\"",
    )?;
    vm.expect_screen(&["certtest2 0", "certtest2 names it in ", "remove 1"])?;
    vm.expect_screen(&["/system/settings/spark/services/certtest2.toml"])?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
