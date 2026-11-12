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
//! pseudo-terminal of the client's type, a dumb one for a client with
//! no TERM, a shell on it runs what is typed and ends with its status,
//! one connection carries six sessions one after another as a
//! ControlMaster runs them, LANG is passed on and other variables are
//! not; root, bob, the key behind an option and a key not
//! listed are refused, and so is alice's password. *sftp* puts a file,
//! renames it, gets it back whole, and makes and removes a folder; *scp*
//! and *scp -O* copy one there and back. *ssh -W* has alice's forwarder
//! connect to the host's port 80, whose keys come back, and by name to
//! the guest's own sshd. last(1) shows alice's sessions from the host,
//! and the files copied are hers. *ssh -R* forwards port 7000 of the
//! guest to a server of the host's: a port below 1024 is refused, the
//! port listens on the loopback alone, a MiB goes each way, and it
//! closes with the client (ADR 0349). A sshd_config with a mistake is
//! refused by *sshd -t*, and a reload keeps what sshd had; one that
//! turns passwords on but keeps bob out, and forwarding to the client
//! alone, is read on a reload: alice's password signs her in, a wrong
//! one and bob's are refused, lastb(1) shows the wrong one (ADR 0348),
//! and *ssh -W* is refused. *service sshd disable* stops sshd and closes
//! the port.

use std::io::{Read, Write};
use std::net::TcpListener;
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
    about: "sshd and service enable against the host's OpenSSH client through a port QEMU forwards: keys from authorized_keys, one behind an option passed over; the host key made and checked; commands with their output, error and exit status, 4 MiB through cat, a terminal and a shell on it, one for a client with no TERM, sessions one after another over a ControlMaster's connection, LANG passed on; root, another person, an unsafe file and a key not listed refused; the guest's own client signing in; last(1); sftp, scp and scp -O copying files there and back; ssh -W to an address and a name; ssh -R with a MiB each way, on the loopback alone, a low port refused; sshd_config with a mistake refused and kept out of a reload; passwords turned on by a reload, a wrong one and a denied user refused, lastb(1), forwarding cut to one way; service disable closing the port (ADRs 0347 to 0349)",
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
/// the host's turn, then before its turn with passwords, and after it.
const BEFORE: &[&str] = &["accounts", "keys", "on"];
const THEN: &[&str] = &["guest", "host"];
const FORWARDING: &[&str] = &["forwarding"];
const PASSWORDS: &[&str] = &["passwords"];
const AFTER: &[&str] = &["off"];

/// The size of what goes through `cat` and back.
const BIG_SIZE: usize = 4 * 1024 * 1024;

/// The size of each file sftp and scp copy: sshdtest checks it.
const COPIED_SIZE: usize = 256 * 1024;

/// The guest's port the host forwards to its server, and how much goes
/// each way through it: sshdtest uses both.
const FORWARDED_PORT: u16 = 7000;
const FORWARDED_SIZE: usize = 1024 * 1024;

/// alice's password and bob's, as sshdtest makes their accounts.
const ALICE_PASSWORD: &str = "wonderland";
const BOB_PASSWORD: &str = "looking-glass";

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
    )?;
    // What ssh asks for a password: the one the test gives it.
    write(&dir.join("askpass"), "#!/bin/sh\nprintf '%s\\n' \"$SSHD_TEST_PASSWORD\"\n", true)
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

/// How a client signs in.
#[derive(Clone, Copy)]
enum Auth<'a> {
    /// With this key of the test's.
    Key(&'a str),
    /// With this password, which ssh asks the test's askpass for.
    Password(&'a str),
}

impl Client<'_> {
    /// `program`, one of OpenSSH's clients, to the guest: with the port
    /// flag `port`, the host key the guest showed, and `auth`.
    fn client(&self, program: &str, port: &str, auth: Auth) -> Command {
        let known = self.dir.join("known_hosts");
        let mut command = Command::new(program);
        command
            .args(["-F", "/dev/null", port, &self.port.to_string()])
            .args(["-o", "StrictHostKeyChecking=yes", "-o"])
            .arg(format!("UserKnownHostsFile={}", known.display()))
            .args(["-o", "LogLevel=ERROR", "-o", "ConnectTimeout=30"]);
        match auth {
            Auth::Key(key) => {
                command
                    .arg("-i")
                    .arg(self.dir.join(key))
                    .args(["-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes"]);
            }
            Auth::Password(password) => {
                command
                    .args(["-o", "PubkeyAuthentication=no", "-o"])
                    .args(["PreferredAuthentications=password", "-o", "NumberOfPasswordPrompts=1"])
                    .env("SSH_ASKPASS", self.dir.join("askpass"))
                    .env("SSH_ASKPASS_REQUIRE", "force")
                    .env("SSHD_TEST_PASSWORD", password);
            }
        }
        command
    }

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
        let mut command = self.client("ssh", "-p", Auth::Key(key));
        command.args(options).arg(format!("{user}@127.0.0.1")).args(args).envs(env.iter().copied());
        let what = format!("ssh {user}@ {args:?}");
        run(command, input, &what)
    }

    /// `ssh` as `user` with `password`, running `command`.
    fn ssh_password(&self, password: &str, user: &str, command: &str) -> Result<Ran, String> {
        let mut ssh = self.client("ssh", "-p", Auth::Password(password));
        ssh.arg(format!("{user}@127.0.0.1")).arg(command);
        run(ssh, b"", &format!("ssh {user}@ with a password, {command:?}"))
    }

    /// `sftp` as alice with her key, running the commands in `batch`.
    fn sftp(&self, batch: &Path) -> Result<Ran, String> {
        let mut sftp = self.client("sftp", "-P", Auth::Key("alice"));
        sftp.arg("-b").arg(batch).arg("alice@127.0.0.1");
        run(sftp, b"", "sftp")
    }

    /// `scp` with `options`, as alice with her key, from `from` to `to`.
    fn scp(&self, options: &[&str], from: &str, to: &str) -> Result<(), String> {
        let mut scp = self.client("scp", "-P", Auth::Key("alice"));
        scp.args(options).args([from, to]);
        let ran = run(scp, b"", &format!("scp {options:?} {from} {to}"))?;
        if ran.status != Some(0) {
            return Err(format!(
                "scp {options:?} {from} {to} ended with {:?}: {}",
                ran.status,
                ran.err.trim()
            ));
        }
        Ok(())
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

/// Runs `command` given `input`, for two minutes at most.
fn run(mut command: Command, input: &[u8], what: &str) -> Result<Ran, String> {
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|err| {
        format!("the sshd test needs the host's OpenSSH client (openssh-client): {what}: {err}")
    })?;
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
        Ok(Err(err)) => return Err(format!("running {what}: {err}")),
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
            return Err(format!("{what} took more than two minutes: {came}"));
        }
    };
    let _ = feeder.join();
    Ok(Ran {
        status: output.status.code(),
        out: output.stdout,
        err: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
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
    // A client with no TERM of its own, as one cron runs, gets a terminal
    // too, a dumb one.
    let none = [("TERM", "")];
    let ran = client.ssh("alice", "alice", &["-tt"], &["echo term-$TERM"], b"", &none)?;
    if ran.status != Some(0) || !ran.out().contains("term-dumb") {
        return Err(format!(
            "a client with no TERM got {:?} ({:?}): {}",
            ran.out(),
            ran.status,
            ran.err.trim()
        ));
    }
    // One connection carries session after session, as a client's
    // ControlMaster runs them: more in all than the four it holds at
    // once. The master stays in the background until -O exit.
    let control = format!("ControlPath={}", client.dir.join("control").display());
    let master = ["-o", "ControlMaster=yes", "-o", &control, "-o", "ControlPersist=60"];
    let ran = client.ssh("alice", "alice", &master, &["echo mux-0"], b"", &[])?;
    if ran.status != Some(0) || !ran.out().contains("mux-0") {
        return Err(format!("the master's session said {:?} ({:?})", ran.out(), ran.status));
    }
    let shared = ["-o", "ControlMaster=no", "-o", &control];
    for n in 1..=6 {
        let ran = client.ssh("alice", "alice", &shared, &[&format!("echo mux-{n}")], b"", &[])?;
        if ran.status != Some(0) || !ran.out().contains(&format!("mux-{n}")) || !ran.err.is_empty()
        {
            return Err(format!(
                "session {n} over the master said {:?} ({:?}): {}",
                ran.out(),
                ran.status,
                ran.err.trim()
            ));
        }
    }
    let exit = ["-o", &control, "-O", "exit"];
    client.ssh("alice", "alice", &exit, &[], b"", &[])?;
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
    // A password is not asked for: sshd_config does not turn them on.
    let ran = client.ssh_password(ALICE_PASSWORD, "alice", "true")?;
    if ran.status != Some(255) || !ran.err.contains("Permission denied (publickey)") {
        return Err(format!("alice's password was not refused: {:?}, {}", ran.status, ran.err));
    }
    copies(client)?;
    // Forwarding: alice's forwarder connects to the host's port 80, whose
    // keys come back, and to the guest's own sshd by name.
    let ran = client.ssh("alice", "alice", &["-W", "10.0.2.100:80"], &[], b"", &[])?;
    if ran.status != Some(0) || !ran.out().contains(" host-alice\n") {
        return Err(format!(
            "ssh -W to the host's port 80 ended with {:?} and wrote {:?}, {}",
            ran.status,
            ran.out(),
            ran.err.trim()
        ));
    }
    let ran = client.ssh("alice", "alice", &["-W", "localhost:22"], &[], b"", &[])?;
    if !ran.out().starts_with("SSH-2.0-Dukovo") {
        return Err(format!(
            "ssh -W to the guest's localhost port 22 wrote {:?}, {}",
            ran.out(),
            ran.err.trim()
        ));
    }
    // The last: the guest goes on.
    client.alice("echo done > ~/host-done", 0, "")?;
    Ok(())
}

/// The bytes of `path`.
fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|err| format!("reading {}: {err}", path.display()))
}

/// sftp and scp, which runs sftp too, and scp -O, which runs scp on the
/// guest: each copies a file there and back whole, as alice's.
fn copies(client: &Client) -> Result<(), String> {
    let dir = client.dir;
    let data: Vec<u8> = (0..COPIED_SIZE).map(|i| (i * 7 % 253) as u8).collect();
    let up = dir.join("up.bin");
    std::fs::write(&up, &data).map_err(|err| format!("writing {}: {err}", up.display()))?;
    let down = dir.join("sftp-back.bin");
    let batch = dir.join("batch");
    write(
        &batch,
        &format!(
            "put {} up.bin\nrename up.bin sftp.bin\nget sftp.bin {}\nmkdir folder\nrmdir \
             folder\nls\n",
            up.display(),
            down.display()
        ),
        false,
    )?;
    let ran = client.sftp(&batch)?;
    if ran.status != Some(0) || !ran.out().contains("sftp.bin") || read(&down)? != data {
        return Err(format!(
            "sftp ended with {:?} and wrote {:?}, {}",
            ran.status,
            ran.out(),
            ran.err.trim()
        ));
    }
    let up_text = up.display().to_string();
    for (options, name) in [(&[][..], "scp"), (&["-O"][..], "scp-o")] {
        let back = dir.join(format!("{name}-back.bin"));
        client.scp(options, &up_text, &format!("alice@127.0.0.1:{name}.bin"))?;
        client.scp(options, &format!("alice@127.0.0.1:{name}.bin"), &back.display().to_string())?;
        if read(&back)? != data {
            return Err(format!("scp {options:?} did not bring the file back whole"));
        }
    }
    Ok(())
}

/// A byte of what goes through the forwarded port: from the guest, or
/// `back` to it, as sshdtest's.
fn pattern(i: usize, back: bool) -> u8 {
    if back { ((i * 13 + 5) % 251) as u8 } else { (i * 7 % 253) as u8 }
}

/// The host's server for the forwarded port: it takes one connection,
/// reads to its end, and sends a MiB back once a MiB came right.
fn serve(listener: &TcpListener) -> Result<(), String> {
    let (mut socket, _) = listener.accept().map_err(|err| format!("accepting: {err}"))?;
    let failed = |err: std::io::Error| format!("the forwarded connection: {err}");
    socket.set_read_timeout(Some(Duration::from_secs(120))).map_err(failed)?;
    let mut came = Vec::new();
    socket.read_to_end(&mut came).map_err(failed)?;
    let right = came.iter().enumerate().all(|(i, &byte)| byte == pattern(i, false));
    if came.len() != FORWARDED_SIZE || !right {
        return Err(format!("{} bytes came through the forwarded port, not the MiB sent", came.len()));
    }
    let back: Vec<u8> = (0..FORWARDED_SIZE).map(|i| pattern(i, true)).collect();
    socket.write_all(&back).map_err(failed)
}

/// The host forwards the guest's port 7000 to a server of its own, with
/// ssh -R, once a port below 1024 is refused; sshdtest sends a MiB
/// through it and gets one back, then says so, and the client goes.
fn remote(client: &Client, vm: &mut Vm) -> Result<(), String> {
    let refused = ["-N", "-o", "ExitOnForwardFailure=yes", "-R", "80:127.0.0.1:1"];
    let ran = client.ssh("alice", "alice", &refused, &[], b"", &[])?;
    if ran.status != Some(255) || !ran.err.contains("forwarding failed for listen port 80") {
        return Err(format!("port 80 was not refused: {:?}, {}", ran.status, ran.err.trim()));
    }
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|err| format!("no port for the server: {err}"))?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();
    let server = std::thread::spawn(move || serve(&listener));
    let mut ssh = client.client("ssh", "-p", Auth::Key("alice"));
    ssh.args(["-N", "-o", "ExitOnForwardFailure=yes", "-R"])
        .arg(format!("{FORWARDED_PORT}:127.0.0.1:{port}"))
        .arg("alice@127.0.0.1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = ssh.spawn().map_err(|err| format!("running ssh -R: {err}"))?;
    let forwarded = vm.expect("sshdtest: forwarded");
    let _ = child.kill();
    let said = child
        .wait_with_output()
        .map_or_default(|output| String::from_utf8_lossy(&output.stderr).into_owned());
    forwarded.map_err(|err| format!("{err}; ssh -R said {:?}", said.trim()))?;
    // The guest had its MiB back: the server is done.
    server.join().map_err(|_| "the forwarded port's server failed".to_string())?
}

/// The host's turn with passwords, once sshd_config turns them on but
/// keeps bob out, and forwards to the client alone.
fn passwords(client: &Client) -> Result<(), String> {
    let ran = client.ssh_password(ALICE_PASSWORD, "alice", "id -un")?;
    if ran.status != Some(0) || ran.out() != "alice\n" {
        return Err(format!(
            "alice's password did not sign her in: {:?}, {:?}, {}",
            ran.status,
            ran.out(),
            ran.err.trim()
        ));
    }
    for (password, user, why) in [
        ("not-wonderland", "alice", "a wrong password"),
        (BOB_PASSWORD, "bob", "bob's, as DenyUsers names him"),
    ] {
        let ran = client.ssh_password(password, user, "true")?;
        if ran.status != Some(255) || !ran.err.contains("Permission denied") {
            return Err(format!("{why} was not refused: {:?}, {}", ran.status, ran.err.trim()));
        }
    }
    // Her key signs her in still; forwarding from the client is refused.
    client.alice("echo key", 0, "key\n")?;
    let ran = client.ssh("alice", "alice", &["-W", "10.0.2.100:80"], &[], b"", &[])?;
    // The client says no more with LogLevel ERROR; the guest checks sshd's
    // log for why.
    if ran.status != Some(255) || !ran.err.contains("stdio forwarding failed") {
        return Err(format!("ssh -W was not refused: {:?}, {}", ran.status, ran.err.trim()));
    }
    // The last, with her password: the guest goes on.
    let ran = client.ssh_password(ALICE_PASSWORD, "alice", "echo done > ~/passwords-done")?;
    if ran.status != Some(0) {
        return Err(format!("alice's last command ended with {:?}: {}", ran.status, ran.err));
    }
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
    let client = Client { dir: &dir, port };
    host(&client)?;
    for part in FORWARDING {
        vm.expect_ahead(&format!("sshdtest: {part}"))?;
    }
    remote(&client, vm)?;
    for part in PASSWORDS {
        vm.expect_ahead(&format!("sshdtest: {part}"))?;
    }
    vm.expect("sshdtest: passwords on")?;
    passwords(&client)?;
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
