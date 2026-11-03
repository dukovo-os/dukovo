// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! sshd(8) against the host's OpenSSH client (ADR 0347). The test makes
//! three Ed25519 keys in out/test/sshd, and QEMU's user network tells
//! two of them at 10.0.2.100 port 80. _/system/tests/posix/sshdtest_
//! makes alice's and bob's accounts and writes alice's authorized_keys:
//! the first key, the second behind an option, which is passed over, and
//! a key of the guest's own; bob's holds the first key too, in a file
//! others may write. alice can neither turn sshd on nor ask authd about
//! keys. *service sshd enable* starts sshd, which makes its host key and
//! listens on port 22; the guest's own client signs in as alice and root
//! is refused. QEMU forwards a port of the host's loopback to port 22,
//! and the host's client, checking the host key the guest showed, signs
//! in as alice: a command's output, error and exit status come back
//! apart, 4 MiB through *cat* come back whole, a terminal is a
//! pseudo-terminal of the client's type, a shell on it runs what is
//! typed and ends with its status, LANG is passed on and other
//! variables are not; root, bob, the key behind an option and a key not
//! listed are refused. last(1) shows alice's sessions from the host, and
//! *service sshd disable* stops sshd and closes the port.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use super::super::*;

/// The keys' folder, from the top of the tree, which QEMU runs in.
const DIR: &str = "out/test/sshd";

/// The test's network: QEMU's user network, with the keys at 10.0.2.100
/// port 80.
const NETDEV: &str = "user,id=n0,guestfwd=tcp:10.0.2.100:80-cmd:out/test/sshd/keys";

pub const CASE: Case = Case {
    name: "sshd",
    about: "sshd and service enable against the host's OpenSSH client through a port QEMU forwards: keys from authorized_keys, one behind an option passed over; the host key made and checked; commands with their output, error and exit status, 4 MiB through cat, a terminal and a shell on it, LANG passed on; root, another person, an unsafe file and a key not listed refused; the guest's own client signing in; last(1); service disable closing the port (ADR 0347)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // Every service, as on a machine, netd setting the card up by DHCP.
    cmdline: "lockdep -- --services --posix --names --network /system/tests/posix/sshdtest",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", NETDEV, "-device", "virtio-net-pci,netdev=n0"],
    script: sshd,
    disks: &[],
    second: None,
};

/// The parts sshdtest prints before it shows the host key, then before
/// the host's turn, and after it.
const BEFORE: &[&str] = &["accounts", "keys", "on"];
const THEN: &[&str] = &["guest", "host"];
const AFTER: &[&str] = &["off"];

/// The size of what goes through `cat` and back.
const BIG_SIZE: usize = 4 * 1024 * 1024;

fn write(path: &Path, text: &str, executable: bool) -> Result<(), String> {
    std::fs::write(path, text).map_err(|err| format!("writing {}: {err}", path.display()))?;
    if executable {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|err| format!("making {} a program: {err}", path.display()))?;
    }
    Ok(())
}

/// The keys: alice's, the one behind an option, and a stranger's; and
/// what QEMU runs for port 80.
fn keys(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|err| format!("removing {}: {err}", dir.display()))?;
    }
    std::fs::create_dir_all(dir).map_err(|err| format!("making {}: {err}", dir.display()))?;
    for name in ["alice", "optioned", "stranger"] {
        let made = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", &format!("host-{name}"), "-f", name])
            .current_dir(dir)
            .status()
            .map_err(|err| {
                format!("the sshd test needs the host's ssh-keygen: install openssh-client ({err})")
            })?;
        if !made.success() {
            return Err(format!("ssh-keygen could not make the {name} key"));
        }
    }
    let at = dir.display();
    write(
        &dir.join("keys"),
        &format!("#!/bin/sh\nexec cat {at}/alice.pub {at}/optioned.pub\n"),
        true,
    )
}

/// What the host's ssh did: its exit status, output and error.
struct Ran {
    status: Option<i32>,
    out: Vec<u8>,
    err: String,
}

impl Ran {
    fn out(&self) -> String {
        String::from_utf8_lossy(&self.out).into_owned()
    }
}

/// The host's client, to the guest's sshd through `port`.
struct Client<'a> {
    dir: &'a Path,
    port: u16,
}

impl Client<'_> {
    /// `ssh` as `user` with the key `key`, with `options` and `args`
    /// after them, given `input`, for two minutes at most.
    fn ssh(
        &self,
        key: &str,
        user: &str,
        options: &[&str],
        args: &[&str],
        input: &[u8],
        env: &[(&str, &str)],
    ) -> Result<Ran, String> {
        let port = self.port.to_string();
        let key = self.dir.join(key);
        let known = self.dir.join("known_hosts");
        let mut command = Command::new("ssh");
        command
            .args(["-F", "/dev/null", "-p", &port, "-i"])
            .arg(&key)
            .args(["-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes"])
            .args(["-o", "StrictHostKeyChecking=yes", "-o"])
            .arg(format!("UserKnownHostsFile={}", known.display()))
            .args(["-o", "LogLevel=ERROR", "-o", "ConnectTimeout=30"])
            .args(options)
            .arg(format!("{user}@127.0.0.1"))
            .args(args)
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child =
            command.spawn().map_err(|err| format!("the sshd test needs the host's ssh: {err}"))?;
        let mut stdin = child.stdin.take();
        let input = input.to_vec();
        // The input goes in while the output comes out.
        let feeder = std::thread::spawn(move || {
            if let Some(stdin) = &mut stdin {
                let _ = stdin.write_all(&input);
            }
        });
        let id = child.id();
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(child.wait_with_output());
        });
        let output = match receive.recv_timeout(Duration::from_secs(120)) {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => return Err(format!("running ssh: {err}")),
            Err(_) => {
                let _ = Command::new("kill").args(["-KILL", &id.to_string()]).status();
                // What came before, to tell where it stopped.
                let came = match receive.recv_timeout(Duration::from_secs(5)) {
                    Ok(Ok(output)) => format!(
                        "{:?} {:?}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ),
                    _ => "nothing".into(),
                };
                return Err(format!("ssh {user}@ {args:?} took more than two minutes: {came}"));
            }
        };
        let _ = feeder.join();
        Ok(Ran {
            status: output.status.code(),
            out: output.stdout,
            err: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// alice's `command`, which must exit with `status` and write `out`.
    fn alice(&self, command: &str, status: i32, out: &str) -> Result<Ran, String> {
        let ran = self.ssh("alice", "alice", &[], &[command], b"", &[])?;
        if ran.status != Some(status) || ran.out() != out {
            return Err(format!(
                "`{command}` as alice ended with {:?} and wrote {:?} ({}), not {status} and {out:?}",
                ran.status,
                ran.out(),
                ran.err.trim()
            ));
        }
        Ok(ran)
    }

    /// Signing in as `user` with `key` is refused.
    fn refused(&self, key: &str, user: &str, why: &str) -> Result<(), String> {
        let ran = self.ssh(key, user, &[], &["true"], b"", &[])?;
        if ran.status != Some(255) || !ran.err.contains("Permission denied") {
            return Err(format!(
                "{user} with the {key} key was not refused, {why}: {:?}, {}",
                ran.status,
                ran.err.trim()
            ));
        }
        Ok(())
    }
}

/// The host's turn: its client against the guest's sshd.
fn host(client: &Client) -> Result<(), String> {
    // The guest's address, once netd has it: QEMU forwards to the
    // address its DHCP server gives, from 10.0.2.2.
    let mut tries = 0;
    loop {
        let ran = client.ssh("alice", "alice", &[], &["id -un; echo $SSH_CLIENT"], b"", &[])?;
        if ran.status == Some(0) {
            let out = ran.out();
            if !out.starts_with("alice\n10.0.2.2 ") {
                return Err(format!("alice's session said {out:?}"));
            }
            break;
        }
        tries += 1;
        if tries > 20 {
            return Err(format!("the host's client did not sign in as alice: {}", ran.err.trim()));
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    // Output and error apart, and the exit status.
    let ran = client.alice("echo out; echo err >&2; exit 7", 7, "out\n")?;
    if !ran.err.contains("err") {
        return Err(format!("the error came back as {:?}", ran.err));
    }
    // 4 MiB through cat and back, whole: the client sends only as much as
    // the session took.
    let big: Vec<u8> = (0..BIG_SIZE).map(|i| (i % 251) as u8).collect();
    let ran = client.ssh("alice", "alice", &[], &["cat"], &big, &[])?;
    if ran.status != Some(0) || ran.out != big {
        return Err(format!(
            "cat gave back {} bytes of {BIG_SIZE}, ending with {:?} ({})",
            ran.out.len(),
            ran.status,
            ran.err.trim()
        ));
    }
    // A terminal of the client's type.
    let env = [("TERM", "xterm-256color")];
    let ran = client.ssh("alice", "alice", &["-tt"], &["tty; echo $TERM"], b"", &env)?;
    let out = ran.out();
    if ran.status != Some(0) || !out.contains("/devices/pts/") || !out.contains("xterm-256color") {
        return Err(format!("the terminal said {out:?} ({:?})", ran.status));
    }
    // A shell on it runs what is typed, and ends with its status. The
    // terminal is a dumb one: the shell's line editor would ask a smart
    // one where its cursor is, which ssh's input, not a terminal, cannot
    // answer.
    let dumb = [("TERM", "dumb")];
    let ran =
        client.ssh("alice", "alice", &["-tt"], &[], b"echo shell-$((6*7))\nexit 3\n", &dumb)?;
    if ran.status != Some(3) || !ran.out().contains("shell-42") {
        return Err(format!("the shell said {:?} and ended with {:?}", ran.out(), ran.status));
    }
    // LANG is passed on; another variable is not.
    let options = ["-o", "SendEnv=LANG", "-o", "SetEnv=FOO=bar"];
    let ran = client.ssh(
        "alice",
        "alice",
        &options,
        &["echo \"$LANG ${FOO-unset}\""],
        b"",
        &[("LANG", "de_DE.UTF-8")],
    )?;
    if ran.out() != "de_DE.UTF-8 unset\n" {
        return Err(format!("the settings came as {:?} ({})", ran.out(), ran.err.trim()));
    }
    client.refused("alice", "root", "as root signs in with no key")?;
    client.refused("alice", "bob", "as others may write bob's authorized_keys")?;
    client.refused("optioned", "alice", "as its line has an option")?;
    client.refused("stranger", "alice", "as it is not listed")?;
    // The last: the guest goes on.
    client.alice("echo done > ~/host-done", 0, "")?;
    Ok(())
}

fn sshd(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let dir = std::env::current_dir().map_err(|err| err.to_string())?.join(DIR);
    keys(&dir)?;
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_ahead("spark: started /system/tests/posix/sshdtest as process ")?;
    for part in BEFORE {
        vm.expect(&format!("sshdtest: {part}"))?;
    }
    let key = program_line(vm, "sshdtest: host key ")?;
    if !key.starts_with("ssh-ed25519 ") {
        return Err(format!("the guest showed \"{key}\" as its host key"));
    }
    for part in THEN {
        vm.expect(&format!("sshdtest: {part}"))?;
    }
    vm.expect("sshdtest: ready for the host")?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map_err(|err| format!("no free port: {err}"))?
        .port();
    write(&dir.join("known_hosts"), &format!("[127.0.0.1]:{port} {key}\n"), false)?;
    vm.monitor(&format!("hostfwd_add n0 tcp:127.0.0.1:{port}-:22"))?;
    host(&Client { dir: &dir, port })?;
    for part in AFTER {
        vm.expect_ahead(&format!("sshdtest: {part}"))?;
    }
    let code = program_line(vm, "(/system/tests/posix/sshdtest) exited with code ")?;
    let log = vm.log();
    let failed: Vec<&str> =
        log.lines().filter(|line| line.contains("sshdtest: FAILED")).map(str::trim).collect();
    if !failed.is_empty() || !log.contains("sshdtest: every check passed") || code != "0x0" {
        return Err(format!("the sshd test failed ({code}): {}", failed.join("; ")));
    }
    Ok(())
}
