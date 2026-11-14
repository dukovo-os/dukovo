// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Terminal of the web console, Dukovo Admin (ADR 0517). Alice, the
//! owner, makes a terminal through the console's route and drives its
//! shell over the console's WebSocket, as the page does; bob, who may use
//! the console and is no administrator, tries what is not his. A Python 3
//! client of the standard library holds each socket, since the host has
//! neither curl for a WebSocket nor a library of its own.
//!
//! The terminal is alice's shell: `tty` names a pseudo-terminal of the
//! machine's, `id -un` says alice and `echo $TERM` says xterm-256color.
//! Resized over the socket to 100 by 30, it says so to `stty size`. A page
//! that closes its socket and comes back attaches with the offset it has
//! read to, and gets no byte again and none missing; one that has nothing
//! gets a replay with the earlier output. A second pane, as Split makes
//! it, has a pseudo-terminal of its own: its `tty` is another, and what is
//! typed in it shows in it and not in the first. On the serial console, `who`
//! lists alice on the terminal's `pts/N` and `last` from the host's
//! address, 10.0.2.2.
//!
//! bob's attach to alice's pane is ADM-131, and so is his ending of her
//! terminal; his own list is empty. An administrator's pane is ADM-106
//! while alice's session is locked. Unlocked, it is root's shell, which the
//! command `id -u` shows, and locking the console ends it: the socket
//! gets `ended` with `locked` at once, the pane is gone from the list, and
//! authd's audit has the shell's start and its end, "the console locked".
//!
//! A socket is refused with the console's own error, and none is opened,
//! when its request has no Origin, another Origin, a request from another
//! site, no token among the protocols, another session's token or no
//! cookie, or brings an API token, which is for the actions alone; and a
//! fifth socket of one session is. A frame the console must not take
//! closes the socket: unmasked 1002, over 64 KiB 1009, not UTF-8 1007, and
//! a message that is not the protocol's 1008.
//!
//! The settings, `[terminal]` in admin.toml, are read again at `service
//! admind reload`: with `sessions-per-account = 2` a third terminal is
//! ADM-130; with `allowed = "administrators"` bob has ADM-132 and a session
//! with `"terminal": false`. The file is put back as it was.
//!
//! The flow of output: a socket that acknowledges nothing is sent less than
//! 400 KiB of the 1.3 MB that `seq 1 200000` writes, and no more while it
//! waits; once it acknowledges, every byte comes, in order, and the part
//! after the command's echo is the one the host computes, by its SHA-256.
//! The pane stays attached. Last, `service admind restart` closes each
//! socket, ends the shells (`who` has none of the console's terminals),
//! and the console that comes back has no terminals for alice.
//!
//! What the pane's commands print reaches the host through nc, to a port on
//! the host's loopback that QEMU's user network gives the guest at
//! 10.0.2.2.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::console_client::{
    ALICE_PASSWORD, Browser, Console, Headers, Pane, Reply, Socket, elevated, tail,
};
use super::super::*;

/// The password of bob, whom alice makes: he may sign in to the console
/// and is no administrator.
const BOB_PASSWORD: &str = "looking-glass";

/// The settings file of the console, and of its Terminal.
const SETTINGS: &str = "/system/settings/admin.toml";

/// Where admind's log and authd's audit say the host's requests come from.
const PEER: &str = "10.0.2.2";

/// The size of a pane as the page opens it, and as the test resizes it.
const SIZE: (u32, u32) = (80, 24);
const BIGGER: (u32, u32) = (100, 30);

/// How long the client reads after the shell draws its prompt again before
/// it types: longer than nukesh's two waits of 2 s for a cursor answer.
const SETTLE: u64 = 5;

/// The most a socket is sent while it acknowledges nothing: 256 KiB held
/// back, the chunks already on their way and what a replay brought.
const HELD_BACK: u64 = 400 * 1024;

/// How many lines `seq` writes in the test of flow control: 200000 lines
/// of up to 8 bytes are 1.3 MB.
const LINES: u32 = 200_000;

/// The test.
pub const CASE: Case = Case {
    name: "admin-terminal",
    about: "The web console's Terminal over its WebSocket, held by a Python 3 client: alice's terminal is her shell on a pseudo-terminal, resized to 100 by 30, that a page which closes its socket attaches to again by the offset it has read to, with no byte twice and none lost, and a new one by a replay; a second pane has a pseudo-terminal of its own, and what is typed in it shows only in it; who and last list it from the host's address; bob cannot attach to it or end it (ADM-131) and sees none; an administrator pane is ADM-106 while locked, root's shell when unlocked, and ends with the lock, which authd's audit records; a socket with no Origin, another, from another site, with no token, another session's token, no cookie or an API token is refused with the console's error and opens nothing, as is a fifth; an unmasked frame closes the socket with 1002, one over 64 KiB with 1009, bad UTF-8 with 1007 and a message that is not the protocol's with 1008; sessions-per-account and allowed in admin.toml take effect at reload (ADM-130, ADM-132) and are put back; a socket that acknowledges nothing is sent under 400 KiB of 1.3 MB of output, and with acknowledgements every byte comes in order, by its SHA-256, with the pane still attached; admind's restart closes each socket and ends the shells (ADR 0517)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, admind among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: admin_terminal,
    disks: &[],
    second: None,
};

fn admin_terminal(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_anywhere("admind: answers at ")?;
    let mut test = Test {
        console: Console::open(vm)?,
        pane: Pane::open()?,
        alice: Browser::default(),
        bob: Browser::default(),
    };
    signs_in(vm, &mut test)?;
    let (shell, mut socket) = types_in_a_terminal(vm, &mut test)?;
    is_resized(&shell, &mut socket)?;
    comes_back(vm, &mut test, &shell, socket)?;
    a_second_pane(vm, &mut test, &shell)?;
    shows_in_who_and_last(vm, &test, &shell)?;
    is_not_bobs(vm, &mut test, &shell)?;
    administrator_pane(vm, &mut test)?;
    refused_handshakes(vm, &mut test)?;
    broken_frames(vm, &mut test)?;
    settings(vm, &mut test)?;
    flow_of_output(vm, &mut test, &shell)?;
    restart(vm, &mut test, &shell)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Who asks the console.
#[derive(Clone, Copy)]
enum Who {
    Alice,
    Bob,
}

/// What the test keeps from one step to the next.
struct Test {
    console: Console,
    pane: Pane,
    alice: Browser,
    bob: Browser,
}

impl Test {
    /// A read of `path`, as the page makes it, whatever the answer.
    fn get(&mut self, who: Who, path: &str) -> Result<Reply, String> {
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        };
        self.console.get(browser, path)
    }

    /// A change of `path` with `body`, as the page makes it, whatever the
    /// answer.
    fn post(&mut self, who: Who, path: &str, body: &Value) -> Result<Reply, String> {
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        };
        self.console.post(browser, path, body)
    }

    /// What `path` says, which must be 200.
    fn read(&mut self, who: Who, path: &str) -> Result<Value, String> {
        self.get(who, path)?.must(path, 200)?.json()
    }

    /// Signs `who` in as `name`, as the page does, with a session of its
    /// own, and keeps the token its page reads from it. The session, as it
    /// reads.
    fn sign_in(&mut self, who: Who, name: &str, password: &str) -> Result<Value, String> {
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        };
        *browser = Browser::default();
        self.console.sign_in(browser, name, password)?;
        self.console.read_session(browser)
    }

    /// A socket of `who`'s, opened as the page opens it, which must be
    /// answered 101 with the right accept key.
    fn socket(&self, vm: &Vm, who: Who) -> Result<Socket, String> {
        let browser = match who {
            Who::Alice => &self.alice,
            Who::Bob => &self.bob,
        };
        let (socket, answer) = Socket::open(vm, &self.console, &browser.socket_headers())?;
        if answer["status"] != json!(101) || answer["accept_ok"] != json!(true) {
            return Err(format!("the console's answer to the socket's handshake is {answer}"));
        }
        Ok(socket)
    }

    /// The terminals of `who`'s, as `GET /api/1/terminals` lists them.
    fn terminals(&mut self, who: Who) -> Result<Value, String> {
        self.read(who, "/api/1/terminals")
    }

    /// How many sockets alice's session holds open, as the list says.
    fn sockets_open(&mut self) -> Result<u64, String> {
        let list = self.terminals(Who::Alice)?;
        list["sockets"]["open"].as_u64().ok_or_else(|| format!("no sockets in {list}"))
    }

    /// Waits up to 15 seconds for alice's session to hold `wanted` sockets
    /// open: a socket that is closed is forgotten a moment later.
    fn sockets_settle(&mut self, wanted: u64) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let open = self.sockets_open()?;
            if open == wanted {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("alice's session holds {open} sockets, not {wanted}"));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Makes a terminal of `who`'s, of the size a page opens one with and
    /// with an administrator's pane if `admin`, and gives the answer,
    /// whatever it is.
    fn make(&mut self, who: Who, admin: bool) -> Result<Reply, String> {
        let body = json!({"cols": SIZE.0, "rows": SIZE.1, "admin": admin});
        self.post(who, "/api/1/terminals", &body)
    }

    /// Waits up to `seconds` for the console to answer again, after its
    /// restart.
    fn answers_again(&self, seconds: u64) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while self.console.get(&mut Browser::default(), "/api/1/health").map(|r| r.status)
            != Ok(200)
        {
            if Instant::now() >= deadline {
                return Err(format!("the console does not answer {seconds} s after its restart"));
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        Ok(())
    }
}

/// A terminal of alice's and its one pane.
struct Shell {
    terminal: u64,
    pane: u64,
    /// The pseudo-terminal the pane's shell has, as `who` names it,
    /// `pts/N`: known once the shell has said it.
    tty: String,
}

/// The terminal that a route answered with, as `{"terminal": {...}}`.
fn shell_of(made: &Value) -> Result<Shell, String> {
    let terminal = made["terminal"]["id"].as_u64();
    let pane = made["terminal"]["panes"][0]["id"].as_u64();
    match (terminal, pane) {
        (Some(terminal), Some(pane)) => Ok(Shell { terminal, pane, tty: String::new() }),
        _ => Err(format!("{made} is no terminal with a pane")),
    }
}

/// Step 1: alice, the owner, signs in, as the other cases do it, and her
/// session says that she may use the Terminal. bob, who may sign in to the
/// console and is no administrator, is made in her elevated pane, as the
/// case `admin` makes carol.
fn signs_in(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    elevated(vm)?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'echo {BOB_PASSWORD} | user add bob; user set --yes --console \
             bob'"
        ),
    )?;
    // The account's hash takes a few seconds without KVM: the log says when
    // the last change is done, and the screen follows.
    vm.expect("authd: changed the account bob")?;
    vm.expect_screen(&["Made bob"])?;
    let session = test.sign_in(Who::Alice, "alice", ALICE_PASSWORD)?;
    if session["terminal"] != json!(true) || session["admin"] != json!(true) {
        return Err(format!(
            "alice's session is {session}, not an administrator's with a terminal"
        ));
    }
    Ok(())
}

/// Step 2: a terminal made through the route, and a socket attached to its
/// pane: `tty`, `id -un` and `echo $TERM` give a pseudo-terminal of the
/// machine's, alice and xterm-256color. The page's size is the pane's.
fn types_in_a_terminal(vm: &Vm, test: &mut Test) -> Result<(Shell, Socket), String> {
    let made = test.make(Who::Alice, false)?.must("alice's terminal", 200)?.json()?;
    let mut shell = shell_of(&made)?;
    if made["terminal"]["panes"][0]["admin"] != json!(false) {
        return Err(format!("alice's terminal has an administrator's pane: {made}"));
    }
    let mut socket = test.socket(vm, Who::Alice)?;
    let attached = socket.attach(shell.pane, SIZE, None)?;
    if attached["terminal"] != json!(shell.terminal)
        || attached["admin"] != json!(false)
        || attached["replay"] != json!(true)
        || attached["live"].as_u64() < attached["offset"].as_u64()
    {
        return Err(format!("the pane's attached message is {attached}"));
    }
    // The shell starts and draws its prompt, which asks where the cursor
    // is; the client answers that in an answer message, as the page's
    // screen does.
    socket.wait_output(shell.pane, "$", 90)?;
    // The console answers a ping with a pong of the same words.
    socket.ping("is anyone there")?;
    socket.wait_pong("is anyone there", 10)?;
    socket.type_in(shell.pane, "tty; id -un; echo $TERM\r")?;
    socket.wait_output(shell.pane, "\r\nalice\r\nxterm-256color\r\n", 60)?;
    let held = socket.output(shell.pane)?;
    let number = tty_before(&held.text, "alice\r\nxterm-256color\r\n").ok_or_else(|| {
        format!("tty did not name a pseudo-terminal: {:?}", tail(&held.text, 300))
    })?;
    shell.tty = format!("pts/{number}");
    Ok((shell, socket))
}

/// Step 3: the page resizes the pane over the socket to 100 by 30, and the
/// shell's `stty size` says 30 100.
fn is_resized(shell: &Shell, socket: &mut Socket) -> Result<(), String> {
    let (cols, rows) = BIGGER;
    socket.text(&json!({"resize": shell.pane, "cols": cols, "rows": rows}))?;
    // The shell draws its prompt again for the new size and asks where the
    // cursor is, waiting up to 2 s for each answer, and keys typed while it
    // waits are lost. A query that more drawing follows is one the client
    // does not answer (it takes it for a replayed one), so the client reads
    // for longer than two such waits before it types.
    socket.wait_quiet(SETTLE)?;
    // nukesh marks where a command's output begins (OSC 133;C) just after
    // the line typed, so a command's first line follows that mark and not
    // a line's end: an empty `echo` first puts the answer on a line of its
    // own.
    socket.type_in(shell.pane, "echo; stty size\r")?;
    socket.wait_output(shell.pane, "\r\n30 100\r\n", 30)
}

/// Step 4: the page closes its socket and opens another. One that asks for
/// what comes after the end it had read to gets `replay: false`, starts at
/// that offset and has only what the shell wrote since; one that asks for
/// nothing gets a replay, which holds the output from before.
fn comes_back(vm: &Vm, test: &mut Test, shell: &Shell, mut first: Socket) -> Result<(), String> {
    // The prompt is drawn and nothing more comes: the end is where it is.
    first.wait_quiet(1)?;
    let end = first.output(shell.pane)?.end();
    first.close(1000)?;
    let code = first.wait_closed(10)?;
    if code != 1000 {
        return Err(format!("the console answered the page's close with {code}, not 1000"));
    }
    drop(first);

    let mut second = test.socket(vm, Who::Alice)?;
    let attached = second.attach(shell.pane, BIGGER, Some(end))?;
    // Only a replay has `modes`, which put the page's screen as the pane's.
    if attached["replay"] != json!(false)
        || attached["offset"].as_u64() != Some(end)
        || attached.get("modes").is_some()
    {
        return Err(format!("attach since {end} is answered {attached}"));
    }
    // As after a resize: what the attach makes the shell draw is over first.
    second.wait_quiet(SETTLE)?;
    second.type_in(shell.pane, "echo; echo after\r")?;
    second.wait_output(shell.pane, "\r\nafter\r\n", 30)?;
    second.wait_quiet(1)?;
    let held = second.output(shell.pane)?;
    if held.first != Some(end) {
        return Err(format!("the output since {end} begins at {:?}", held.first));
    }
    for earlier in ["xterm-256color", "/devices/pts/", "30 100"] {
        if held.text.contains(earlier) {
            return Err(format!(
                "the output since {end} has {earlier:?} from before: {:?}",
                held.text
            ));
        }
    }
    second.close(1000)?;
    second.wait_closed(10)?;
    drop(second);

    let mut third = test.socket(vm, Who::Alice)?;
    let attached = third.attach(shell.pane, BIGGER, None)?;
    if attached["replay"] != json!(true) || !attached["modes"].is_string() {
        return Err(format!("attach with no offset is answered {attached}, not a replay"));
    }
    third.wait_output(shell.pane, "xterm-256color", 30)?;
    third.wait_output(shell.pane, "\r\nafter\r\n", 30)?;
    // The replay makes the shell draw itself again, which it does within a
    // moment; the page that goes away before that leaves it a question.
    third.wait_quiet(2)?;
    third.close(1000)?;
    third.wait_closed(10)?;
    test.sockets_settle(0)
}

/// Step 5: a second pane in alice's terminal, as Split makes it, has a
/// pseudo-terminal of its own: its `tty` is not the first pane's, what is
/// typed in it shows in it, and the first pane's output has none of it.
/// Ending it leaves the first.
fn a_second_pane(vm: &Vm, test: &mut Test, shell: &Shell) -> Result<(), String> {
    let path = format!("/api/1/terminals/{}/panes", shell.terminal);
    let body = json!({"cols": SIZE.0, "rows": SIZE.1});
    let made = test.post(Who::Alice, &path, &body)?.must("alice's second pane", 200)?.json()?;
    let second = made["terminal"]["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pane| pane["id"].as_u64())
        .find(|&id| id != shell.pane)
        .ok_or_else(|| format!("the terminal has no second pane: {made}"))?;
    let mut socket = test.socket(vm, Who::Alice)?;
    // The first pane too, at the size it has, to see what reaches it.
    socket.attach(shell.pane, BIGGER, None)?;
    socket.attach(second, SIZE, None)?;
    socket.wait_output(second, "$", 90)?;
    socket.wait_quiet(SETTLE)?;
    socket.type_in(second, "echo; tty; echo second-pane\r")?;
    socket.wait_output(second, "\r\nsecond-pane\r\n", 60)?;
    let held = socket.output(second)?;
    let number = tty_before(&held.text, "second-pane\r\n").ok_or_else(|| {
        format!("tty in the second pane named no pseudo-terminal: {:?}", tail(&held.text, 300))
    })?;
    if format!("pts/{number}") == shell.tty {
        return Err(format!("both panes have {}", shell.tty));
    }
    let first = socket.output(shell.pane)?;
    if first.text.contains("second-pane") || first.text.contains("Operation not permitted") {
        return Err(format!(
            "the second pane's shell writes to the first pane: {:?}",
            tail(&first.text, 300)
        ));
    }
    let end = format!("/api/1/terminals/panes/{second}/end");
    test.post(Who::Alice, &end, &json!({}))?.must("the second pane's end", 200)?;
    socket.close(1000)?;
    socket.wait_closed(10)?;
    test.sockets_settle(0)
}

/// Step 6: on the serial console, in alice's pane, `who` lists alice on the
/// terminal that `tty` named, and `last` too, from the host's address, as
/// a session that is still there.
fn shows_in_who_and_last(vm: &mut Vm, test: &Test, shell: &Shell) -> Result<(), String> {
    let tty = &shell.tty;
    let who = test.pane.output(vm, "who")?;
    if !who_lists(&who, "alice", tty) {
        return Err(format!("who does not list alice on {tty}: {who:?}"));
    }
    let last = test.pane.output(vm, "last")?;
    if !last_lists(&last, "alice", tty, PEER) {
        return Err(format!("last does not list alice on {tty} from {PEER}: {last:?}"));
    }
    Ok(())
}

/// Step 7: bob signs in. His socket's attach to alice's pane is an `error`
/// with ADM-131, as is his ending of her terminal, and his own list has no
/// terminal; hers is as it was.
fn is_not_bobs(vm: &Vm, test: &mut Test, shell: &Shell) -> Result<(), String> {
    let session = test.sign_in(Who::Bob, "bob", BOB_PASSWORD)?;
    if session["admin"] != json!(false) || session["terminal"] != json!(true) {
        return Err(format!("bob's session is {session}"));
    }
    let mut socket = test.socket(vm, Who::Bob)?;
    socket.text(&json!({"attach": shell.pane, "cols": SIZE.0, "rows": SIZE.1}))?;
    let said = socket.wait_said("error", 15)?;
    if said["error"]["id"] != json!("ADM-131") || said["pane"] != json!(shell.pane) {
        return Err(format!("bob's attach to alice's pane is answered {said}"));
    }
    let path = format!("/api/1/terminals/{}/end", shell.terminal);
    test.post(Who::Bob, &path, &json!({}))?.refused(
        "bob's ending of alice's terminal",
        404,
        "ADM-131",
    )?;
    let list = test.terminals(Who::Bob)?;
    if list["terminals"] != json!([]) {
        return Err(format!("bob's list of terminals is {list}"));
    }
    let hers = test.terminals(Who::Alice)?;
    if hers["terminals"][0]["id"] != json!(shell.terminal) {
        return Err(format!("alice's terminal is gone after bob's try: {hers}"));
    }
    drop(socket);
    Ok(())
}

/// Step 8: an administrator's pane is ADM-106 while alice's session is
/// locked. She unlocks, and it is made: `id -u` in it says 0. Locking the
/// console ends it, with `locked`, within 5 seconds; the list has no pane
/// of it; and authd's audit has the shell's start, for alice from the
/// host's address on its terminal, and its end, "the console locked".
fn administrator_pane(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.make(Who::Alice, true)?.refused("an administrator's pane while locked", 403, "ADM-106")?;
    let unlock = json!({"kind": "password", "password": ALICE_PASSWORD});
    test.post(Who::Alice, "/api/1/unlock", &unlock)?.must("alice's unlock", 200)?;
    let made = test.make(Who::Alice, true)?.must("an administrator's pane", 200)?.json()?;
    let mut admin = shell_of(&made)?;
    if made["terminal"]["panes"][0]["admin"] != json!(true) {
        return Err(format!("the administrator's terminal is {made}"));
    }
    let mut socket = test.socket(vm, Who::Alice)?;
    let attached = socket.attach(admin.pane, SIZE, None)?;
    if attached["admin"] != json!(true) {
        return Err(format!("the administrator's pane is attached as {attached}"));
    }
    socket.wait_output(admin.pane, "#", 90)?;
    socket.type_in(admin.pane, "tty; id -u\r")?;
    socket.wait_output(admin.pane, "\r\n0\r\n", 60)?;
    let held = socket.output(admin.pane)?;
    let number = tty_before(&held.text, "0\r\n").ok_or_else(|| {
        format!("tty did not name a pseudo-terminal: {:?}", tail(&held.text, 300))
    })?;
    admin.tty = format!("pts/{number}");

    test.post(Who::Alice, "/api/1/lock", &json!({}))?.must("alice's lock", 200)?;
    let ended = socket.wait_said("ended", 5)?;
    if ended["ended"] != json!(admin.pane) || ended["why"] != json!("locked") {
        return Err(format!("the administrator's pane ended as {ended}, not locked"));
    }
    // The status is not known: the message leaves it out, and has no null.
    if ended.get("status").is_some() {
        return Err(format!("the ended message has a status that admind does not know: {ended}"));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let list = test.terminals(Who::Alice)?;
        let held: Vec<&Value> = list["terminals"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|terminal| terminal["panes"].as_array().into_iter().flatten())
            .filter(|pane| pane["id"] == json!(admin.pane))
            .collect();
        if held.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!("the list still has the administrator's pane: {list}"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    drop(socket);
    administrator_audit(vm, test, &admin.tty)
}

/// Waits up to 90 seconds for authd's audit to have the start of the
/// administrator's shell on `tty` and its end by the lock: logd takes a
/// while to give authd's lines to the log.
fn administrator_audit(vm: &mut Vm, test: &Test, tty: &str) -> Result<(), String> {
    elevated(vm)?;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let audit = test.pane.output(
            vm,
            "elevate log --audit | grep -e 'console administrator shell' -e 'console terminal'",
        )?;
        if let Some(process) = audit_start(&audit, tty)
            && audit_end(&audit, process)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "authd's audit has no start and end of the administrator's shell on {tty}: \
                 {audit:?}"
            ));
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// Step 9: each handshake that the console must refuse is answered with its
/// own error and no 101, and the session's sockets are as many as before.
/// Then the fifth socket of a session is ADM-130.
fn refused_handshakes(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let proof = json!({"proof": ALICE_PASSWORD, "name": "terminal", "scopes": ["*"], "days": 1});
    let made = test.post(Who::Alice, "/api/1/account/tokens/add", &proof)?;
    let token = made.must("alice's API token", 200)?.json()?["token"]
        .as_str()
        .ok_or("no token was made")?
        .to_string();
    let others = test.bob.token.clone().ok_or("bob has no token")?;
    let page = test.alice.socket_headers();
    let with_token = |token: &str| format!("dukovo-terminal-1, csrf.{token}");
    let protocols = "Sec-WebSocket-Protocol";
    let refusals = [
        ("no Origin", page.clone().without("Origin"), 403, "ADM-109"),
        ("another Origin", page.clone().with("Origin", "https://evil.example"), 403, "ADM-109"),
        (
            "a request from another site",
            page.clone().with("Sec-Fetch-Site", "cross-site"),
            403,
            "ADM-109",
        ),
        (
            "no token among the protocols",
            page.clone().with(protocols, "dukovo-terminal-1"),
            400,
            "ADM-108",
        ),
        (
            "another session's token",
            page.clone().with(protocols, &with_token(&others)),
            403,
            "ADM-109",
        ),
        ("no cookie", page.clone().without("Cookie"), 401, "ADM-107"),
        (
            "an API token",
            page.clone().without("Cookie").with("Authorization", &format!("Bearer {token}")),
            403,
            "ADM-124",
        ),
    ];
    let before = test.sockets_open()?;
    for (what, headers, status, id) in refusals {
        refused(vm, test, what, &headers, status, id)?;
        let open = test.sockets_open()?;
        if open != before {
            return Err(format!("{what}: the session holds {open} sockets, not {before}"));
        }
    }

    // Four sockets are all a session may hold.
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(test.socket(vm, Who::Alice)?);
    }
    test.sockets_settle(4)?;
    refused(vm, test, "a fifth socket", &page, 409, "ADM-130")?;
    drop(held);
    test.sockets_settle(0)
}

/// Opens a socket with `headers`, which the console must refuse with the
/// error `id` and `status`: no 101 and a body that is the error.
fn refused(
    vm: &Vm,
    test: &Test,
    what: &str,
    headers: &Headers,
    status: u64,
    id: &str,
) -> Result<(), String> {
    let (socket, answer) = Socket::open(vm, &test.console, headers)?;
    let (got, said) = refusal(&answer);
    if got != Some(status) || said.as_deref() != Some(id) {
        return Err(format!("{what}: the socket was answered {answer}, not {id} with {status}"));
    }
    drop(socket);
    Ok(())
}

/// What the test sends to break a rule of the protocol.
enum Break {
    /// Bytes written to the connection as they are.
    Bytes(Vec<u8>),
    /// A message of the protocol's own kind, JSON in a masked text frame.
    Message(Value),
    /// A masked text frame with this text.
    Text(&'static str),
}

/// Step 10: on a fresh socket each, what the console must not take closes
/// the socket with the code the protocol says: an unmasked frame 1002, a
/// binary message of 70 KiB 1009, a text frame whose payload is not UTF-8
/// 1007, and a text message that is not the protocol's, or no JSON at all,
/// 1008.
fn broken_frames(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let key = [0x37, 0xfa, 0x21, 0x3d];
    let cases = [
        ("an unmasked frame", Break::Bytes(vec![0x81, 0x02, 0x7b, 0x7d]), 1002),
        ("a message of 70 KiB", Break::Bytes(masked(0x2, &vec![0; 70 * 1024], key)), 1009),
        ("text that is not UTF-8", Break::Bytes(masked(0x1, &[0xff, 0xfe], key)), 1007),
        (
            "a message that is not the protocol's",
            Break::Message(json!({"attach": 1, "shell": "sh"})),
            1008,
        ),
        ("text that is no JSON", Break::Text("not JSON"), 1008),
    ];
    for (what, broken, wanted) in cases {
        let mut socket = test.socket(vm, Who::Alice)?;
        match broken {
            Break::Bytes(bytes) => socket.frame(&bytes)?,
            Break::Message(message) => socket.text(&message)?,
            Break::Text(text) => socket.text_raw(text)?,
        }
        let code = socket.wait_closed(15)?;
        if code != wanted {
            return Err(format!("{what}: the console closed the socket with {code}, not {wanted}"));
        }
        drop(socket);
    }
    test.sockets_settle(0)
}

/// Step 11: as root in the pane, a `[terminal]` table is appended to
/// admin.toml, and admind reads it again at `service admind reload`. With
/// `sessions-per-account = 2` and two terminals of alice's, a third is
/// ADM-130. With `allowed = "administrators"` bob's list is ADM-132 and his
/// session has `"terminal": false`; alice still may. The file is put back
/// as it was, and read again.
fn settings(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let original = test.pane.output(vm, &format!("cat {SETTINGS}"))?;
    if has_terminal_table(&original) {
        return Err(format!("{SETTINGS} has a [terminal] table before the test: {original:?}"));
    }
    if !original.is_ascii() {
        return Err(format!("{SETTINGS} is not ASCII, so its length is not known: {original:?}"));
    }
    elevated(vm)?;

    appended(vm, test, "\n[terminal]\nsessions-per-account = 2\n")?;
    reloaded(vm, test)?;
    let list = test.terminals(Who::Alice)?;
    if list["most"] != json!(2) {
        return Err(format!("sessions-per-account = 2 leaves the most terminals as {list}"));
    }
    let have = list["terminals"].as_array().map_or(0, Vec::len);
    if have > 2 {
        return Err(format!("alice has {have} terminals before the third: {list}"));
    }
    for _ in have..2 {
        test.make(Who::Alice, false)?.must("alice's second terminal", 200)?;
    }
    test.make(Who::Alice, false)?.refused("alice's third terminal", 409, "ADM-130")?;

    appended(vm, test, "allowed = \"administrators\"\n")?;
    reloaded(vm, test)?;
    test.get(Who::Bob, "/api/1/terminals")?.refused("bob's list of terminals", 403, "ADM-132")?;
    let session = test.read(Who::Bob, "/api/1/session")?;
    if session["terminal"] != json!(false) {
        return Err(format!("bob's session says he has a terminal: {session}"));
    }
    let session = test.read(Who::Alice, "/api/1/session")?;
    if session["terminal"] != json!(true) {
        return Err(format!("alice's session says she has no terminal: {session}"));
    }

    let status =
        test.pane.status(vm, &format!("elevate truncate -s {} {SETTINGS}", original.len()))?;
    if status != 0 {
        return Err(format!("truncate of {SETTINGS} ended with status {status}"));
    }
    let put_back = test.pane.output(vm, &format!("cat {SETTINGS}"))?;
    if put_back != original {
        return Err(format!("{SETTINGS} was {original:?} and is {put_back:?} after the test"));
    }
    reloaded(vm, test)?;
    let list = test.terminals(Who::Alice)?;
    let session = test.read(Who::Bob, "/api/1/session")?;
    if list["most"] != json!(32) || session["terminal"] != json!(true) {
        return Err(format!("the settings put back leave {list} and bob's session {session}"));
    }
    Ok(())
}

/// Adds `text` to the end of admin.toml, as root.
fn appended(vm: &mut Vm, test: &Test, text: &str) -> Result<(), String> {
    // printf writes what its escapes say; the pane types them as they are.
    let typed = text.replace('\n', "\\n");
    let command = format!("printf '{typed}' | elevate tee -a {SETTINGS} > /dev/null");
    let status = test.pane.status(vm, &command)?;
    if status != 0 {
        return Err(format!("appending to {SETTINGS} ended with status {status}"));
    }
    Ok(())
}

/// How many lines admind's log has that say it read its settings and its
/// certificate again, as it does at SIGHUP.
fn reloads(log: &str) -> usize {
    log.lines()
        .filter(|line| line.contains("admind: the certificate ") && line.contains(" is read again"))
        .count()
}

/// Has admind read its settings again, as `service admind reload` asks,
/// and waits up to 30 seconds for its log to say so.
fn reloaded(vm: &mut Vm, test: &Test) -> Result<(), String> {
    let before = reloads(&vm.log());
    let status = test.pane.status(vm, "elevate service admind reload")?;
    if status != 0 {
        return Err(format!("service admind reload ended with status {status}"));
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while reloads(&vm.log()) <= before {
        if Instant::now() >= deadline {
            return Err("admind's log does not say it read its settings again in 30 s".into());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

/// Step 12: in a terminal of alice's, with no acknowledgement, `seq 1
/// 200000` writes about 1.3 MB, of which the socket is sent under 400 KiB
/// and no more while it waits. Once it acknowledges, all of it comes, and
/// the SHA-256 of the part after the command's echo is the host's for the
/// text that `seq` writes; the pane is still attached.
fn flow_of_output(vm: &Vm, test: &mut Test, shell: &Shell) -> Result<(), String> {
    let pane = shell.pane;
    let mut socket = test.socket(vm, Who::Alice)?;
    socket.attach(pane, BIGGER, None)?;
    // The replay has the shell draw itself again, as after a resize.
    socket.wait_quiet(SETTLE)?;
    // An empty `echo` first, so that seq's first line follows a line's end
    // and not the mark nukesh puts after the line typed (OSC 133;C).
    socket.type_in(pane, &format!("echo; seq 1 {LINES}\r"))?;
    socket.wait_bytes(pane, 256 * 1024, 120)?;
    let held = socket.wait_quiet(5)?.length(pane);
    if held >= HELD_BACK {
        return Err(format!(
            "with nothing acknowledged, {held} bytes were sent, not under {HELD_BACK}"
        ));
    }
    let still = socket.wait_quiet(3)?.length(pane);
    if still != held {
        return Err(format!("{} more bytes came while none were acknowledged", still - held));
    }

    socket.acks(true)?;
    let last = format!("\r\n{}\r\n{LINES}\r\n", LINES - 1);
    socket.wait_output(pane, &last, 180)?;
    let text = socket.output(pane)?.text;
    let wanted = seq_text(LINES);
    let got = seq_part(&text, wanted.len())
        .ok_or_else(|| format!("the output has no seq after the echo: {:?}", tail(&text, 200)))?;
    if Sha256::digest(got.as_bytes()) != Sha256::digest(wanted.as_bytes()) {
        let at = first_difference(got, &wanted).unwrap_or(0);
        return Err(format!(
            "seq's output differs from the host's at byte {at}: the pane has {:?}, the host {:?}",
            got.get(at.saturating_sub(10)..(at + 20).min(got.len())),
            wanted.get(at.saturating_sub(10)..(at + 20).min(wanted.len()))
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let list = test.terminals(Who::Alice)?;
        let attached = list["terminals"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|terminal| terminal["panes"].as_array().into_iter().flatten())
            .find(|held| held["id"] == json!(pane))
            .map(|held| held["attached"].clone());
        if attached == Some(json!(1)) {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!("the flooded pane is attached {attached:?} times, not once"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    drop(socket);
    Ok(())
}

/// Step 13: as root in the pane, `service admind restart`: each socket
/// closes (1001, or the connection ends), `who` has none of the console's
/// terminals, and once alice signs in again, her session having ended with
/// the restart, she has no terminals.
fn restart(vm: &mut Vm, test: &mut Test, shell: &Shell) -> Result<(), String> {
    let mut attached = test.socket(vm, Who::Alice)?;
    attached.attach(shell.pane, BIGGER, None)?;
    let mut idle = test.socket(vm, Who::Alice)?;
    elevated(vm)?;
    let status = test.pane.status(vm, "elevate service admind restart")?;
    if status != 0 {
        return Err(format!("service admind restart ended with status {status}"));
    }
    for (what, socket) in [("the attached socket", &mut attached), ("the idle socket", &mut idle)] {
        let code = socket.wait_closed(30)?;
        if code != 1001 && code != 0 {
            return Err(format!(
                "{what} was closed with {code}, not 1001 or by the connection ending"
            ));
        }
    }
    // The test's own shell, where alice signed in at the machine's
    // console, is on a pseudo-terminal too, and stays.
    let own = test.pane.output(vm, "tty")?;
    let own = own.trim().rsplit_once("/pts/").map(|(_, number)| format!("pts/{number}"));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let who = test.pane.output(vm, "who")?;
        let mut names = console_terminals(&who);
        names.retain(|name| Some(name) != own.as_ref());
        if names.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!("who lists {names:?} 30 s after admind's restart: {who:?}"));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    // The sessions are gone with the old admind; the console is up again
    // within a minute and signs alice in once more.
    test.answers_again(60)?;
    test.sign_in(Who::Alice, "alice", ALICE_PASSWORD)?;
    let list = test.terminals(Who::Alice)?;
    if list["terminals"] != json!([]) {
        return Err(format!("alice has terminals after the restart: {list}"));
    }
    Ok(())
}

/// The `N` of the `/devices/pts/N` that `tty` printed just before `then`,
/// the lines that followed it, as the pane wrote them: the last one, if
/// the output has more than one.
fn tty_before(text: &str, then: &str) -> Option<String> {
    let after = format!("\r\n{then}");
    text.match_indices("/devices/pts/")
        .filter_map(|(at, name)| {
            let rest = &text[at + name.len()..];
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            (digits > 0 && rest[digits..].starts_with(&after)).then(|| rest[..digits].to_string())
        })
        .last()
}

/// The terminals of a `who` listing that are the console's: `pts/N`.
fn console_terminals(who: &str) -> Vec<String> {
    who.lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter(|terminal| terminal.starts_with("pts/"))
        .map(str::to_string)
        .collect()
}

/// Whether the listing of `who` has `user` on `terminal`: it prints the
/// user, the terminal and the time, and no address.
fn who_lists(who: &str, user: &str, terminal: &str) -> bool {
    who.lines().any(|line| {
        let mut fields = line.split_whitespace();
        fields.next() == Some(user) && fields.next() == Some(terminal)
    })
}

/// Whether the listing of `last` has `user` on `terminal` from `from`, as a
/// session that is still there: the name, the terminal, the address and the
/// time, and `still logged in`.
fn last_lists(last: &str, user: &str, terminal: &str, from: &str) -> bool {
    last.lines().any(|line| {
        let mut fields = line.split_whitespace();
        fields.next() == Some(user)
            && fields.next() == Some(terminal)
            && fields.next() == Some(from)
            && line.trim_end().ends_with("still logged in")
    })
}

/// The process of the administrator's shell on `tty` that authd's audit
/// says started for alice from the host's address: the number that ends
/// the line.
fn audit_start(audit: &str, tty: &str) -> Option<u64> {
    let start = format!("console administrator shell started for alice from {PEER} on {tty},");
    let line = audit.lines().find(|line| line.contains(&start))?;
    line.trim_end().rsplit("process ").next()?.parse().ok()
}

/// Whether authd's audit says alice's administrator shell `process` ended
/// because the console locked.
fn audit_end(audit: &str, process: u64) -> bool {
    let ended = format!(
        "alice's console administrator shell, process {process}, ended: the console locked"
    );
    audit.lines().any(|line| line.contains(&ended))
}

/// Whether admin.toml has a `[terminal]` table of its own, which is no
/// comment.
fn has_terminal_table(settings: &str) -> bool {
    settings.lines().any(|line| line.trim_start().starts_with("[terminal]"))
}

/// The status and the error's id of a refused handshake, from what the
/// client answered: the status line's, and the id in the JSON body.
fn refusal(answer: &Value) -> (Option<u64>, Option<String>) {
    let body: Option<Value> =
        answer["body"].as_str().and_then(|body| serde_json::from_str(body).ok());
    let id = body.and_then(|body| body["error"]["id"].as_str().map(str::to_string));
    (answer["status"].as_u64(), id)
}

/// A frame of a client's, final and masked with `key`: what a page's
/// browser sends, as the console must take it.
fn masked(opcode: u8, payload: &[u8], key: [u8; 4]) -> Vec<u8> {
    let mut frame = vec![0x80 | opcode];
    match payload.len() {
        len @ 0..=125 => frame.push(0x80 | len as u8),
        len @ 126..=0xffff => {
            frame.push(0x80 | 0x7e);
            frame.extend((len as u16).to_be_bytes());
        }
        len => {
            frame.push(0x80 | 0x7f);
            frame.extend((len as u64).to_be_bytes());
        }
    }
    frame.extend(key);
    frame.extend(payload.iter().enumerate().map(|(at, byte)| byte ^ key[at % 4]));
    frame
}

/// What `seq 1 last` writes on a terminal: each number and a line end of
/// CR LF, as the terminal writes one.
fn seq_text(last: u32) -> String {
    (1..=last).fold(String::new(), |mut text, number| {
        // Writing to a string does not fail.
        let _ = write!(text, "{number}\r\n");
        text
    })
}

/// The `length` bytes of a pane's output that are `seq`'s: they begin at
/// its first line, after the line end that the empty `echo` before it
/// writes.
fn seq_part(output: &str, length: usize) -> Option<&str> {
    let at = output.find("\r\n1\r\n2\r\n3\r\n")? + 2;
    output.get(at..at + length)
}

/// The offset of the first byte at which `got` and `wanted` differ.
fn first_difference(got: &str, wanted: &str) -> Option<usize> {
    let same = got.bytes().zip(wanted.bytes()).take_while(|(a, b)| a == b).count();
    (same < got.len().max(wanted.len())).then_some(same)
}

#[cfg(test)]
mod tests {
    use super::super::super::console_client::hex;
    use super::*;

    #[test]
    fn the_terminal_is_read_from_the_routes_answer() {
        let made = json!({"terminal": {"id": 4, "panes": [{"id": 9, "admin": false}]}});
        let shell = shell_of(&made).unwrap();
        assert_eq!((shell.terminal, shell.pane), (4, 9));
        assert!(shell_of(&json!({"terminal": {"id": 4, "panes": []}})).is_err());
        assert!(shell_of(&json!({"done": true})).is_err());
    }

    #[test]
    fn tty_names_the_pseudo_terminal_before_the_lines_that_follow_it() {
        let output = "tty; id -un\r\n/devices/pts/3\r\nalice\r\nxterm-256color\r\n$ ";
        assert_eq!(tty_before(output, "alice\r\nxterm-256color\r\n").as_deref(), Some("3"));
        // Its command line has no number after the name, and is none.
        assert_eq!(tty_before("tty /devices/pts/\r\n0\r\n", "0\r\n"), None);
        // The last of two, such as a replay brings, is the one.
        let twice = "/devices/pts/3\r\n0\r\n and /devices/pts/12\r\n0\r\n";
        assert_eq!(tty_before(twice, "0\r\n").as_deref(), Some("12"));
        assert_eq!(tty_before("/devices/pts/3\r\n1\r\n", "0\r\n"), None);
        assert_eq!(tty_before("", "0\r\n"), None);
    }

    #[test]
    fn who_and_last_are_read_as_they_print() {
        let who = "alice    tty1         2026-11-14 10:00\n\
                   alice    pts/3        2026-11-14 10:05\n\
                   bob      web          2026-11-14 10:06\n";
        assert!(who_lists(who, "alice", "pts/3"));
        assert!(who_lists(who, "alice", "tty1"));
        assert!(!who_lists(who, "alice", "pts/4"));
        assert!(!who_lists(who, "bob", "pts/3"));
        assert_eq!(console_terminals(who), ["pts/3"]);
        assert!(console_terminals("alice    tty1   2026-11-14 10:00\n").is_empty());
        assert!(console_terminals("").is_empty());

        let last = "alice    pts/3        10.0.2.2         Sat Nov 14 10:05   still logged in\n\
                    alice    pts/2        10.0.2.2         Sat Nov 14 09:00 - 09:30  (00:30)\n";
        assert!(last_lists(last, "alice", "pts/3", "10.0.2.2"));
        // A session that ended is not one that is still there.
        assert!(!last_lists(last, "alice", "pts/2", "10.0.2.2"));
        assert!(!last_lists(last, "alice", "pts/3", "10.0.2.3"));
        assert!(!last_lists(last, "bob", "pts/3", "10.0.2.2"));
    }

    #[test]
    fn the_audit_has_the_start_and_the_end_of_the_shell_by_its_process() {
        let audit = "console terminal started for alice from 10.0.2.2 on pts/3: session process 71\n\
            console administrator shell started for alice from 10.0.2.2 on pts/4, in the web \
            console session AbCd: process 88\n\
            alice's console administrator shell, process 88, ended: the console locked\n";
        assert_eq!(audit_start(audit, "pts/4"), Some(88));
        // pts/4 must not be taken for pts/40, nor the plain terminal's line for it.
        assert_eq!(audit_start(audit, "pts/3"), None);
        assert_eq!(audit_start(audit, "pts/40"), None);
        assert!(audit_end(audit, 88));
        assert!(!audit_end(audit, 8));
        assert!(!audit_end(audit, 71));
        assert!(!audit_end("", 88));
        let other_end = "alice's console administrator shell, process 88, ended: it exited\n";
        assert!(!audit_end(other_end, 88));
    }

    #[test]
    fn only_a_terminal_table_that_is_no_comment_is_a_table() {
        assert!(has_terminal_table("reach = \"local\"\n[terminal]\nallowed = \"everyone\"\n"));
        assert!(has_terminal_table("  [terminal]  # mine\n"));
        assert!(!has_terminal_table("# [terminal]\n# allowed = \"nobody\"\n"));
        assert!(!has_terminal_table("[console]\nreach = \"any\"\n"));
        assert!(!has_terminal_table(""));
    }

    #[test]
    fn admind_s_log_is_counted_for_the_times_it_read_its_settings() {
        let log = "admind: answers at https://x\n\
                   admind: the certificate this-machine is read again\n\
                   admind: the certificate this-machine is read again\n\
                   admind: the certificate this-machine is not read\n";
        assert_eq!(reloads(log), 2);
        assert_eq!(reloads(""), 0);
    }

    #[test]
    fn a_refusal_is_the_status_and_the_id_in_the_body() {
        let answer =
            json!({"status": 403, "headers": {}, "body": "{\"error\":{\"id\":\"ADM-109\"}}"});
        assert_eq!(refusal(&answer), (Some(403), Some("ADM-109".to_string())));
        // A 101 has no body, and a body that is no JSON has no id.
        assert_eq!(refusal(&json!({"status": 101, "accept_ok": true})), (Some(101), None));
        assert_eq!(refusal(&json!({"status": 502, "body": "<html>"})), (Some(502), None));
        assert_eq!(refusal(&json!({})), (None, None));
    }

    #[test]
    fn a_clients_frames_are_masked_as_rfc_6455_shows() {
        // RFC 6455 5.7: "Hello", masked with 37 fa 21 3d.
        assert_eq!(hex(&masked(0x1, b"Hello", [0x37, 0xfa, 0x21, 0x3d])), "818537fa213d7f9f4d5158");
        // 5.2: a length of 126 or more takes two bytes, 64 KiB or more eight.
        let key = [1, 2, 3, 4];
        let frame = masked(0x2, &[0; 256], key);
        assert_eq!(&frame[..8], [0x82, 0xfe, 0x01, 0x00, 1, 2, 3, 4]);
        assert_eq!(frame.len(), 8 + 256);
        assert_eq!(&frame[8..12], [1, 2, 3, 4]);
        let frame = masked(0x2, &vec![0; 70 * 1024], key);
        assert_eq!(&frame[..14], [0x82, 0xff, 0, 0, 0, 0, 0, 0x01, 0x18, 0, 1, 2, 3, 4]);
        assert_eq!(frame.len(), 14 + 70 * 1024);
        // The edges of the shortest form.
        assert_eq!(masked(0x1, &[0; 125], key)[1], 0x80 | 0x7d);
        assert_eq!(masked(0x1, &[0; 126], key)[1], 0x80 | 0x7e);
        assert_eq!(masked(0x1, &vec![0; 65535], key)[1], 0x80 | 0x7e);
        assert_eq!(masked(0x1, &vec![0; 65536], key)[1], 0x80 | 0x7f);
        assert_eq!(masked(0x8, &[], key), [0x88, 0x80, 1, 2, 3, 4]);
    }

    #[test]
    fn seq_is_what_the_host_computes_and_is_found_after_the_echo() {
        assert_eq!(seq_text(3), "1\r\n2\r\n3\r\n");
        // 200000 lines: 9 of 1 digit, 90 of 2, ... 100001 of 6 digits at
        // 100000 to 199999, and 200000, besides the line end of each.
        assert_eq!(
            seq_text(LINES).len(),
            9 * 3 + 90 * 4 + 900 * 5 + 9000 * 6 + 90000 * 7 + 100001 * 8
        );
        // As nukesh writes it: the line typed, the mark of the output's
        // start, the empty echo's line end, then seq's lines.
        let output = format!("$ echo; seq 1 3\r\n\u{1b}]133;C\u{1b}\\\r\n{}$ ", seq_text(3));
        assert_eq!(seq_part(&output, seq_text(3).len()), Some("1\r\n2\r\n3\r\n"));
        assert_eq!(seq_part("$ ls\r\n", 4), None);
        // Output that stops short has no whole part.
        assert_eq!(seq_part("x\r\n1\r\n2\r\n3\r\n", 100), None);
    }

    #[test]
    fn the_first_byte_that_differs_is_found() {
        assert_eq!(first_difference("abc", "abc"), None);
        assert_eq!(first_difference("abc", "abd"), Some(2));
        assert_eq!(first_difference("abc", "ab"), Some(2));
        assert_eq!(first_difference("", "a"), Some(0));
        assert_eq!(first_difference("", ""), None);
    }
}
