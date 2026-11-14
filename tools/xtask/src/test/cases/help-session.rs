// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! help-session(1) lets a helper with no account work in the owner's
//! screen over SSH (ADR 0540), on an installed system with the host's
//! OpenSSH client as the helper, through a port QEMU forwards, as the
//! *sshd* test does. alice, the owner, makes her account at the first
//! start, puts a key of the test's in her authorized_keys and turns sshd
//! on, which passwords do not open: only the help session's code does.
//!
//! *help-session start* at tty1 shows the address, a code of 16 letters
//! and digits and the fingerprint of sshd's host key, which must be the
//! one the host's ssh-keyscan reads. A wrong code is refused. The right
//! one signs the helper in as *help*: tty1 shows the event, and what the
//! helper types, *echo helped > ~/proof*, runs in alice's own shell, so
//! that the file is hers, in her home. The same code is refused a second
//! time. *elevate* typed by the helper is refused with the words that
//! name *help-session allow-elevate*; alice signs in a second time, over
//! SSH with her key, and allows it, which asks her password there, and
//! then elevate runs without one. A *sleep 999 &* the helper leaves runs
//! until alice presses Ctrl-], which ends the session at once: the
//! helper's ssh ends with the words sshd sends, tty1 says why, and no
//! sleep runs any more.
//!
//! *help-session log* lists the session with its end, and with its ID
//! prints the transcript, which holds the start, the helper's sign-in,
//! each line typed, the allowing of elevate and the end. The serial log
//! holds authd's audit line for each step. A session that ends at its
//! five-minute deadline is left to the unit tests of authd, as a test
//! that waits five minutes would take longer than any other.

use std::path::Path;
use std::process::Command;

use super::super::*;

/// The keys' folder, from the top of the tree, which QEMU runs in.
const DIR: &str = "out/test/help-session";

/// alice's password.
pub const PASSWORD: &str = "queen of hearts";

/// What the helper's ssh sees of the guest's address: QEMU's user
/// network forwards from 10.0.2.2.
const FROM: &str = "10.0.2.2";

pub const CASE: Case = Case {
    name: "help-session",
    about: "help-session on an installed system, with the host's ssh as the helper through a forwarded port: start shows the address, a code and sshd's host key, which the host reads the same; a wrong code is refused, the right one signs the helper in as help, tty1 shows it and what the helper types, which runs as alice and leaves her file; the same code is refused again; elevate in the help screen is refused until alice, signed in again, allows it; a sleep left running is gone when Ctrl-] ends the session, the helper's ssh ends with sshd's words, help-session log lists it and prints what was typed, and authd's audit lines tell of each step (ADR 0540)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: help_session,
    disks: &[],
    second: None,
};

/// A port of the host's loopback that nothing uses now.
pub fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|err| format!("no free port: {err}"))
}

/// `command` as root: alice, the owner, is an administrator, and elevate
/// reads her password from its standard input with `-S`.
pub fn as_root(command: &str) -> String {
    format!("echo '{PASSWORD}' | elevate -S {command}")
}

/// Makes the keys' folder `dir` afresh with an Ed25519 key of alice's and
/// the askpass program that answers ssh's password prompt with
/// `$TEST_SSH_PASSWORD`. alice's public key line.
pub fn keys(dir: &Path) -> Result<String, String> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|err| format!("removing {}: {err}", dir.display()))?;
    }
    std::fs::create_dir_all(dir).map_err(|err| format!("making {}: {err}", dir.display()))?;
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "alice", "-f", "alice"])
        .current_dir(dir)
        .status()
        .map_err(|err| {
            format!("the test needs the host's ssh-keygen: install openssh-client ({err})")
        })?;
    if !made.success() {
        return Err("ssh-keygen could not make alice's key".into());
    }
    let askpass = dir.join("askpass");
    std::fs::write(&askpass, "#!/bin/sh\nprintf '%s\\n' \"$TEST_SSH_PASSWORD\"\n")
        .map_err(|err| format!("writing {}: {err}", askpass.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&askpass, std::fs::Permissions::from_mode(0o755))
            .map_err(|err| format!("making {} a program: {err}", askpass.display()))?;
    }
    let public = dir.join("alice.pub");
    let line = std::fs::read_to_string(&public)
        .map_err(|err| format!("reading {}: {err}", public.display()))?;
    Ok(line.trim().to_string())
}

/// The guest's Ed25519 host key as the host's ssh-keyscan reads it
/// through `port`, as a line of known_hosts. Tried for a minute and a
/// half: QEMU takes the connection before sshd listens.
pub fn scan_host_key(port: u16) -> Result<String, String> {
    for _ in 0..30 {
        let scanned = Command::new("ssh-keyscan")
            .args(["-T", "10", "-t", "ed25519", "-p", &port.to_string(), "127.0.0.1"])
            .output()
            .map_err(|err| {
                format!("the test needs the host's ssh-keyscan: install openssh-client ({err})")
            })?;
        let text = String::from_utf8_lossy(&scanned.stdout);
        if let Some(line) = text.lines().find(|line| line.contains(" ssh-ed25519 ")) {
            return Ok(line.to_string());
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    Err("ssh-keyscan found no host key in the guest: sshd does not listen".into())
}

/// The fingerprint in ssh-keygen's `-l` listing `text` of one key, as
/// `SHA256:...`.
fn fingerprint_in(text: &str) -> Option<String> {
    let word = text.split_whitespace().nth(1)?;
    word.starts_with("SHA256:").then(|| word.to_string())
}

/// The fingerprint of the key in the known_hosts line `line`, which
/// ssh-keygen reads from a file in `dir`.
pub fn fingerprint(dir: &Path, line: &str) -> Result<String, String> {
    let file = dir.join("fingerprint.pub");
    std::fs::write(&file, format!("{line}\n"))
        .map_err(|err| format!("writing {}: {err}", file.display()))?;
    let listed = Command::new("ssh-keygen")
        .arg("-lf")
        .arg(&file)
        .output()
        .map_err(|err| format!("the test needs the host's ssh-keygen: {err}"))?;
    let text = String::from_utf8_lossy(&listed.stdout);
    fingerprint_in(&text).ok_or_else(|| format!("ssh-keygen -l said {:?}", text.trim()))
}

/// The host's ssh to the guest through `port`, which takes only the host
/// key in `dir`'s known_hosts, as `ssh-keyscan` read it.
pub fn ssh(dir: &Path, port: u16) -> Command {
    let mut command = Command::new("ssh");
    command
        .args(["-F", "/dev/null", "-p", &port.to_string()])
        .args(["-o", "StrictHostKeyChecking=yes", "-o"])
        .arg(format!("UserKnownHostsFile={}", dir.join("known_hosts").display()))
        .args(["-o", "LogLevel=ERROR", "-o", "ConnectTimeout=30"])
        // The shell's line editor would ask a smart terminal where its
        // cursor is, which ssh's input, not a terminal, cannot answer.
        .env("TERM", "dumb");
    command
}

/// `command` signs in with alice's key, as ssh -i does.
pub fn with_alice_key<'a>(command: &'a mut Command, dir: &Path) -> &'a mut Command {
    command
        .arg("-i")
        .arg(dir.join("alice"))
        .args(["-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes"])
}

/// `command` signs in with `password`, which ssh asks the askpass
/// program of [`keys`] for.
pub fn with_password<'a>(
    command: &'a mut Command,
    dir: &Path,
    password: &str,
) -> &'a mut Command {
    command
        .args(["-o", "PubkeyAuthentication=no", "-o"])
        .args(["PreferredAuthentications=password", "-o", "NumberOfPasswordPrompts=1"])
        .env("SSH_ASKPASS", dir.join("askpass"))
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("TEST_SSH_PASSWORD", password)
}

/// The host's ssh as a client of the guest, on a terminal of the guest's
/// sshd, with what it writes kept to look in: what the guest's side
/// writes comes on its standard output, what ssh itself says on its
/// standard error.
pub struct Session {
    what: String,
    child: Child,
    input: Option<ChildStdin>,
    said: Arc<Mutex<Vec<u8>>>,
}

/// Appends what `from` gives, until it ends, to `to`, in a thread.
fn collect(mut from: impl Read + Send + 'static, to: Arc<Mutex<Vec<u8>>>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = from.read(&mut buf) {
            to.lock().expect("the output lock is not poisoned").extend_from_slice(&buf[..n]);
        }
    });
}

impl Session {
    /// Runs `command`, which `what` says in an error.
    pub fn start(mut command: Command, what: &str) -> Result<Session, String> {
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|err| {
            format!("the test needs the host's OpenSSH client (openssh-client): {what}: {err}")
        })?;
        let said = Arc::new(Mutex::new(Vec::new()));
        let input = child.stdin.take();
        if let Some(out) = child.stdout.take() {
            collect(out, Arc::clone(&said));
        }
        if let Some(err) = child.stderr.take() {
            collect(err, Arc::clone(&said));
        }
        Ok(Session { what: what.to_string(), child, input, said })
    }

    /// What it wrote so far, without carriage returns and escape sequences.
    pub fn said(&self) -> String {
        clean_log(&String::from_utf8_lossy(&self.said.lock().expect("the output is not poisoned")))
    }

    /// The end of what it wrote, for an error.
    fn tail(&self) -> String {
        let said = self.said();
        let from = said.char_indices().rev().nth(400).map_or(0, |(at, _)| at);
        said[from..].trim().to_string()
    }

    /// Types `text` to it.
    pub fn send(&mut self, text: &str) -> Result<(), String> {
        let input = self.input.as_mut().ok_or("the session's input is closed")?;
        input
            .write_all(text.as_bytes())
            .and_then(|()| input.flush())
            .map_err(|err| format!("{}: typing to it: {err}; it said: {}", self.what, self.tail()))
    }

    /// Ends its input, as a person's Ctrl-D does a command that reads it.
    pub fn close_input(&mut self) {
        self.input = None;
    }

    /// Waits up to `seconds` until it has written `needle`.
    pub fn wait_for(&mut self, needle: &str, seconds: u64) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if self.said().contains(needle) {
                return Ok(());
            }
            let ended = self.child.try_wait().map_err(|err| format!("{}: {err}", self.what))?;
            if let Some(status) = ended {
                // The threads that read its output may still hold the end.
                std::thread::sleep(Duration::from_millis(500));
                if self.said().contains(needle) {
                    return Ok(());
                }
                return Err(format!(
                    "{} ended ({status}) without saying \"{needle}\"; it said: {}",
                    self.what,
                    self.tail()
                ));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "{} did not say \"{needle}\" within {seconds} s; it said: {}",
                    self.what,
                    self.tail()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Waits up to `seconds` until it ends: its exit status.
    pub fn finished(&mut self, seconds: u64) -> Result<Option<i32>, String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if let Some(status) = self.child.try_wait().map_err(|err| format!("{}: {err}", self.what))? {
                // The threads that read its output may still hold the end.
                std::thread::sleep(Duration::from_millis(500));
                return Ok(status.code());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "{} still runs after {seconds} s; it said: {}",
                    self.what,
                    self.tail()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // It may have ended already, and a test that fails leaves nothing
        // running on the host.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Signs in as `help` with `code`, with or without a terminal: the
/// command that runs ssh.
fn helper_login(dir: &Path, port: u16, code: &str, terminal: bool) -> Command {
    let mut command = ssh(dir, port);
    with_password(&mut command, dir, code);
    if terminal {
        command.arg("-tt");
    }
    command.arg("help@127.0.0.1");
    command
}

/// A sign-in as `help` with `code`, which `why` names, is refused: ssh
/// ends with status 255, saying permission was denied.
fn refused(dir: &Path, port: u16, code: &str, why: &str) -> Result<(), String> {
    let what = format!("ssh as help with {why}");
    let mut ssh = Session::start(helper_login(dir, port, code, false), &what)?;
    ssh.close_input();
    let status = ssh.finished(60)?;
    if status != Some(255) || !ssh.said().contains("Permission denied") {
        return Err(format!(
            "{why} was not refused: ssh ended with {status:?} and said: {}",
            ssh.tail()
        ));
    }
    Ok(())
}

/// The code a help-session start shows on `screen`, without its dashes.
fn code_on(screen: &[String]) -> Option<String> {
    let line = screen.iter().find_map(|line| line.trim().strip_prefix("the code "))?;
    let code: String = line.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    let letters = |b: u8| b.is_ascii_uppercase() || (b'2'..=b'9').contains(&b);
    (code.len() == 16 && code.bytes().all(letters)).then_some(code)
}

/// The fingerprint of sshd's host key that a help-session start shows on
/// `screen`.
fn key_on(screen: &[String]) -> Option<String> {
    screen.iter().map(|line| line.trim()).find(|line| line.starts_with("SHA256:")).map(String::from)
}

/// The ID of the session that `screen` says the owner ended.
fn ended_id(screen: &[String]) -> Option<String> {
    screen.iter().find_map(|line| {
        let rest = line.trim().strip_prefix("Help session ")?;
        let (id, _) = rest.split_once(" ended: the owner ended it.")?;
        (!id.is_empty() && !id.contains(' ')).then(|| id.to_string())
    })
}

/// `screen`'s lines with each run of blanks one space, so that the
/// padding of a table does not matter.
fn squeezed(screen: &[String]) -> Vec<String> {
    screen.iter().map(|line| line.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
}

/// Waits until the screen shows each of `needles`, its blanks squeezed
/// as [`squeezed`] does, for up to 15 seconds, as [`Vm::expect_screen`]
/// waits.
fn expect_squeezed(vm: &mut Vm, needles: &[&str]) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let shown = squeezed(&vm.screen()?);
        match screen_shows(&shown, needles) {
            Ok(()) => return Ok(()),
            Err(err) if Instant::now() >= deadline => return Err(err),
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

/// alice's account, made at the first start on tty1, which signs her in.
pub fn make_alice(vm: &mut Vm) -> Result<(), String> {
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("authd: 0 accounts; answering at /system/live/authd/socket")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    vm.expect_screen(&[
        &format!("{RELEASE} {VERSION}"),
        "This computer has no account yet: make yours, the owner's,",
    ])?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Full name (may be empty):", "Alice Liddell")?;
    answer_prompt(vm, "New password:", PASSWORD)?;
    answer_prompt(vm, "The same again:", PASSWORD)?;
    vm.expect_anywhere("authd: alice signed in on pts/")?;
    vm.expect_screen(&["Made alice, number 1000, home /users/alice"])?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")
}

/// alice's key in her authorized_keys, and sshd turned on, which writes
/// its host key and listens. The host's `port` reaches it.
pub fn sshd_on(vm: &mut Vm, alice_key: &str, port: u16) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; mkdir -p ~/.ssh; echo '{alice_key}' > ~/.ssh/authorized_keys; echo \"key $((6 \
             * 7))\""
        ),
    )?;
    vm.expect_screen(&["key 42"])?;
    answer_prompt(
        vm,
        "$",
        &format!("clear; {}; echo \"sshd $((6 * 7))\"", as_root("service sshd enable")),
    )?;
    vm.expect_screen(&["sshd 42"])?;
    vm.monitor(&format!("hostfwd_add n0 tcp:127.0.0.1:{port}-:22"))
}

/// Starts alice's help session at tty1 and reads what it gives the
/// helper: the code, once the screen's address and host key are right.
fn start_session(vm: &mut Vm, host_key: &str) -> Result<String, String> {
    answer_prompt(vm, "$", "clear; help-session start")?;
    vm.expect_screen(&[
        "Help session for alice until ",
        "(1 hour). Give your helper:",
        "ssh help@10.0.2.15",
        "Waiting for your helper...",
    ])?;
    let screen = vm.screen()?;
    screen_shows(
        &screen,
        &[
            "They see and type in this screen, as you. Ctrl-] ends it at any time.",
            "Everything they type is kept: help-session log shows it.",
        ],
    )?;
    if key_on(&screen).as_deref() != Some(host_key) {
        return Err(format!(
            "the screen shows the host key {:?}, not {host_key}, which the host's ssh-keyscan reads",
            key_on(&screen)
        ));
    }
    code_on(&screen)
        .ok_or_else(|| format!("the screen shows no code: {}", screen.join(" | ").trim()))
}

/// The helper signs in with the code and does what a helper does: a file
/// of alice's, an elevate that is refused and, once alice allows it,
/// runs, and a sleep left running.
fn help(vm: &mut Vm, dir: &Path, port: u16, code: &str) -> Result<Session, String> {
    // A wrong code is refused, and the audit log says so.
    refused(dir, port, "AAAA-AAAA-AAAA-AAAA", "a wrong code")?;
    let mut helper = Session::start(helper_login(dir, port, code, true), "ssh as help")?;
    helper.wait_for("You are in a help session", 60)?;
    vm.expect_screen(&[&format!("[help-session] Your helper signed in from {FROM} with the code.")])?;

    // What the helper types runs in alice's shell and is on her screen.
    helper.send("echo helped > ~/proof\r")?;
    helper.send("echo \"mine $(stat -c %U ~/proof) $(cat ~/proof)\"\r")?;
    helper.wait_for("mine alice helped", 60)?;
    vm.expect_screen(&["echo helped > ~/proof", "mine alice helped"])?;

    // The code is spent.
    refused(dir, port, code, "the code a second time")?;

    // elevate says what allows it, and runs once alice does.
    helper.send("elevate true; echo \"elevate-status $?\"\r")?;
    helper.wait_for("elevate-status 1", 60)?;
    let said = helper.said();
    if !said.contains("elevate runs only while its owner allows it: help-session allow-elevate") {
        return Err(format!("elevate in the help screen refused with: {}", helper.tail()));
    }
    vm.expect_screen(&["elevate runs only while its owner allows it"])?;
    allow_elevate(vm, dir, port)?;
    helper.send("elevate sh -c 'echo \"elev $(id -un)\"'\r")?;
    helper.wait_for("elev root", 60)?;

    // Left running, until the session ends. The job's process is named
    // sleep only once it has started the program, so the count waits for
    // it, up to 20 seconds.
    helper.send("sleep 999 &\r")?;
    helper.send(
        "n=0; until pgrep -x sleep > /dev/null || [ $n -ge 20 ]; do n=$((n+1)); sleep 1; done; \
         echo \"bg $(pgrep -c -x sleep)\"\r",
    )?;
    helper.wait_for("bg 1", 60)?;
    Ok(helper)
}

/// alice signs in a second time, over SSH with her key, and allows
/// elevate in the help screen: help-session asks her password there.
fn allow_elevate(vm: &mut Vm, dir: &Path, port: u16) -> Result<(), String> {
    let mut command = ssh(dir, port);
    with_alice_key(&mut command, dir).args(["-tt", "alice@127.0.0.1", "help-session allow-elevate"]);
    let mut owner = Session::start(command, "ssh as alice")?;
    owner.wait_for("password for alice:", 60)?;
    owner.send(&format!("{PASSWORD}\r"))?;
    owner.wait_for("elevate runs in your help screen for 15 minutes, without a password.", 60)?;
    owner.finished(30)?;
    vm.expect_screen(&["[help-session] elevate runs in the help screen for 15 minutes."])
}

/// What alice's session shows once she ends it: why, and the ID.
fn after_ctrl_right_bracket(vm: &mut Vm, helper: &mut Session) -> Result<String, String> {
    vm.press(&["ctrl-bracket_right"])?;
    helper.wait_for("[help-session] The help session ended.", 60)?;
    helper.finished(30)?;
    vm.expect_screen(&[
        "ended: the owner ended it.",
        "What your helper changed in your files and settings stays, as your own changes would.",
    ])?;
    ended_id(&vm.screen()?).ok_or_else(|| "tty1 says no ID of the session that ended".into())
}

/// The audit lines authd wrote of session `id`, which the serial log
/// holds.
fn audit_lines(vm: &Vm, id: &str) -> Result<(), String> {
    let log = vm.log();
    let lines = [
        format!("authd: alice started help session {id} until "),
        ", for a helper with a code: shell process ".to_string(),
        format!("authd: a wrong help-session code from {FROM}, for process "),
        format!(
            "authd: a helper signed in to alice's help session {id} from {FROM} with code, for \
             process "
        ),
        format!("authd: alice let elevate run in help session {id} until "),
        format!(
            "authd: help session {id} of alice ended (owner); the helper from {FROM} with code \
             changed what the transcript shows, which stays"
        ),
    ];
    match lines.iter().find(|line| !log.contains(line.as_str())) {
        Some(line) => Err(format!("authd never logged \"{line}\"")),
        None => Ok(()),
    }
}

/// What alice finds once it has ended: nothing left running, the helper's
/// file as hers, the session in the list and its transcript.
fn afterwards(vm: &mut Vm, id: &str) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        "clear; echo \"sleepers $(pgrep -c -x -r RSDT sleep)\"; echo \"proof $(cat ~/proof) \
         $(stat -c %U ~/proof)\"",
    )?;
    vm.expect_screen(&["sleepers 0", "proof helped alice"])?;
    answer_prompt(vm, "$", "clear; help-session log")?;
    expect_squeezed(vm, &["ID Owner Started Ended", &format!("{id} alice ")])?;
    if squeezed(&vm.screen()?).iter().any(|line| line.starts_with(id) && line.contains("running")) {
        return Err(format!("help-session log says that session {id} still runs"));
    }
    answer_prompt(vm, "$", &format!("clear; help-session log {id}"))?;
    expect_squeezed(
        vm,
        &[
            "start alice started it, until ",
            &format!("helper signed in from {FROM} with the code"),
            "typed echo helped > ~/proof",
            "elevate alice let elevate run until ",
            "end owner",
        ],
    )
}

fn help_session(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let dir = std::env::current_dir().map_err(|err| err.to_string())?.join(DIR);
    let alice_key = keys(&dir)?;
    make_alice(vm)?;
    let port = free_port()?;
    sshd_on(vm, &alice_key, port)?;
    let host_key_line = scan_host_key(port)?;
    let known_hosts = dir.join("known_hosts");
    std::fs::write(&known_hosts, format!("{host_key_line}\n"))
        .map_err(|err| format!("writing {}: {err}", known_hosts.display()))?;
    let fingerprint = fingerprint(&dir, &host_key_line)?;

    let code = start_session(vm, &fingerprint)?;
    let mut helper = help(vm, &dir, port, &code)?;
    let id = after_ctrl_right_bracket(vm, &mut helper)?;
    afterwards(vm, &id)?;
    audit_lines(vm, &id)?;
    super::install_uefi::power_off(vm)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(String::from).collect()
    }

    #[test]
    fn the_code_is_read_from_the_line_that_gives_it_without_its_dashes() {
        let screen = lines("Help session for alice until 15:04 (1 hour). Give your helper:\n  ssh help@10.0.2.15\n  the code KQ7R-M2XW-9HTP-4BZE\n");
        assert_eq!(code_on(&screen).as_deref(), Some("KQ7RM2XW9HTP4BZE"));
    }

    #[test]
    fn a_line_that_is_no_code_is_not_taken_for_one() {
        // Too short, a letter the alphabet leaves out, lowercase, and a
        // key's line in its place.
        for line in [
            "  the code KQ7R-M2XW-9HTP",
            "  the code KQ7R-M2XW-9HTP-4BZ0",
            "  the code kq7r-m2xw-9htp-4bze",
            "  only the key SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f signs in",
        ] {
            assert_eq!(code_on(&lines(line)), None, "{line}");
        }
    }

    #[test]
    fn sshds_host_key_is_the_fingerprint_on_a_line_of_its_own() {
        let screen = lines(
            "  this computer's key, which their ssh asks them to check:\n    SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f\nThey see\n",
        );
        assert_eq!(
            key_on(&screen).as_deref(),
            Some("SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f")
        );
        assert_eq!(key_on(&lines("  sshd has no host key yet")), None);
    }

    #[test]
    fn the_id_is_the_word_after_help_session_on_the_line_that_says_it_ended() {
        let screen = lines(
            "Help session for alice until 15:04 (1 hour). Give your helper:\nHelp session k7q2mx ended: the owner ended it. What your helper typed is kept: help-session log k7q2mx.\n",
        );
        assert_eq!(ended_id(&screen).as_deref(), Some("k7q2mx"));
        let deadline = lines("Help session k7q2mx ended: the time was up. What");
        assert_eq!(ended_id(&deadline), None);
    }

    #[test]
    fn a_tables_padding_does_not_matter_once_its_blanks_are_squeezed() {
        let screen = lines("ID      Owner  Started           Ended\nk7q2mx  alice  2026-11-14 12:00  2026-11-14 12:03");
        let shown = squeezed(&screen);
        assert_eq!(shown[0], "ID Owner Started Ended");
        assert!(shown[1].starts_with("k7q2mx alice "));
    }

    #[test]
    fn the_fingerprint_is_the_second_word_of_the_listing() {
        let listing = "256 SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f [127.0.0.1]:2222 (ED25519)\n";
        assert_eq!(
            fingerprint_in(listing).as_deref(),
            Some("SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f")
        );
        assert_eq!(fingerprint_in("not a key"), None);
        assert_eq!(fingerprint_in(""), None);
    }
}
