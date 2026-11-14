// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! help-session(1): lets a helper who needs no account work in a shell of
//! the owner's, on the owner's own screen (ADR 0540).
//!
//! `start` makes the help screen, a pseudo-terminal pair, and a socket
//! pair, and asks authd(8) for a session with the pair's slave side and
//! one end of the socket: authd starts the owner's shell on the terminal
//! in a resource domain of its own, and hands sshd(8) the socket end when
//! the helper signs in as `help` (`authd/help.rs`, and `sshd/src/help.rs`
//! on the other side of the socket). help-session then joins the help
//! screen's output to the owner's terminal and to the socket, and the
//! owner's keys and the helper's to the help screen, until the session
//! ends. Its connection to authd is the session's life line: authd
//! tells the owner's terminal what happens (a helper signed in, time
//! running out, the end) as event lines on it, and ends the session when
//! it closes.
//!
//! `end`, `log` and `allow-elevate` ask authd for the owner's own session
//! from another terminal. The requests and their answers are in
//! `accounts::request`.
//!
//! When a session misbehaves, authd's `help session` lines in the audit
//! log say each step it took, and `help-session log` shows what the
//! helper typed. The boot test `help-session` covers the tool.

use std::ffi::OsStr;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::Ordering;

use accounts::request::{
    Answer, HELP_MINUTES_DEFAULT, HELP_MINUTES_MAX, HELP_MINUTES_MIN, HELP_NAME, HelpEnded,
    HelpEvent, HelpKept, HelpStarted, Request, SSH_KEY_ALGORITHMS, WRONG_PASSWORD, help_code_shown,
};

use crate::args::{Arg, Args};
use crate::elevate::{self, Pty, Raw};
use crate::ip::{self, Interface};
use crate::util::{self, Fail, fail, usage};
use crate::{account, prompt, sshkeys};

/// Ctrl-], the owner's key that ends the session at once. It is telnet's
/// escape too, so a person may know it.
const END_KEY: u8 = 0x1d;

/// How many wrong passwords are taken before help-session gives up, as
/// elevate takes.
const TRIES: u32 = 3;

/// The standard output's descriptor, where the owner's terminal shows the
/// help screen.
const OWNER_OUT: RawFd = libc::STDOUT_FILENO;

/// The most of the help screen's output kept for a helper whose
/// connection takes it slower than the screen writes it. Past it, what is
/// kept is dropped, so that the owner's screen never waits for the
/// helper's, and they are told [`FELL_BEHIND`].
const BEHIND_MAX: usize = 256 * 1024;

/// What a helper whose connection fell behind is told.
const FELL_BEHIND: &str =
    "Your connection fell behind the screen, and some of it was skipped: Ctrl-L redraws it.";

/// The text of `--help`, and of a usage error.
fn usage_text() -> String {
    format!(
        "usage: help-session start [--for DURATION] [--key KEY | --key-file FILE] [--open-ssh]\n\
         \x20      help-session end\n\
         \x20      help-session log [ID]\n\
         \x20      help-session allow-elevate\n\
         \x20      help-session --help | --version\n\
         \n\
         DURATION is 90m, 2h, 1h30m, or 45 for minutes: {HELP_MINUTES_MIN} minutes to {} hours, \
         {HELP_MINUTES_DEFAULT} minutes without --for.",
        HELP_MINUTES_MAX / 60
    )
}

/// help-session(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let words = args.rest();
    if words.iter().any(|word| word == "--help") {
        println!("{}", usage_text());
        return Ok(0);
    }
    if words.iter().any(|word| word == "--version") {
        println!("help-session (Dukovo) {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    let mut args = Args::new(args.tool, words);
    let Some(command) = args.subcommand() else {
        return Err(match args.next()? {
            Some(other) => args.unknown(&other),
            None => usage(usage_text()),
        });
    };
    match command.to_str() {
        Some("start") => start(&start_options(&mut args)?),
        Some("end") => {
            nothing_more(&mut args)?;
            end()
        }
        Some("log") => log(id_operand(&mut args)?.as_deref()),
        Some("allow-elevate") => {
            nothing_more(&mut args)?;
            allow_elevate()
        }
        _ => {
            Err(usage(format!("no such command: {}\n{}", command.to_string_lossy(), usage_text())))
        }
    }
}

/// Fails if anything is left in `args`.
fn nothing_more(args: &mut Args) -> Result<(), Fail> {
    match args.next()? {
        None => Ok(()),
        Some(other) => Err(args.unknown(&other)),
    }
}

/// The session's ID that `args` ends with, if it has one.
fn id_operand(args: &mut Args) -> Result<Option<String>, Fail> {
    let mut id = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if id.is_none() => {
                id = Some(operand.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(id)
}

/// What `help-session start` was asked for.
struct Start {
    minutes: u32,
    /// The helper's key, as its blob, in place of a code.
    key: Option<Vec<u8>>,
    open_ssh: bool,
}

fn start_options(args: &mut Args) -> Result<Start, Fail> {
    let mut minutes = HELP_MINUTES_DEFAULT;
    let (mut line, mut file, mut open_ssh) = (None, None, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "for" => {
                minutes = parse_minutes(&args.text()?).map_err(usage)?;
            }
            Arg::Long(name) if name == "key" => line = Some(args.text()?),
            Arg::Long(name) if name == "key-file" => file = Some(args.value()?),
            Arg::Long(name) if name == "open-ssh" => open_ssh = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let key = match (line, file) {
        (Some(_), Some(_)) => return Err(usage("give --key or --key-file, not both".into())),
        (Some(line), None) => Some(key_from_line(&line).map_err(fail)?),
        (None, Some(file)) => Some(key_from_file(&file)?),
        (None, None) => None,
    };
    Ok(Start { minutes, key, open_ssh })
}

/// The minutes `text` says: `90m`, `2h`, `1h30m`, or a bare number of
/// minutes; within [`HELP_MINUTES_MIN`] and [`HELP_MINUTES_MAX`].
fn parse_minutes(text: &str) -> Result<u32, String> {
    let total = minutes_in(text).ok_or_else(|| {
        format!("'{text}' is not a length of time: write it as 90m, 2h, 1h30m, or 45 for minutes")
    })?;
    u32::try_from(total)
        .ok()
        .filter(|m| (HELP_MINUTES_MIN..=HELP_MINUTES_MAX).contains(m))
        .ok_or_else(|| {
            format!(
                "a help session lasts {HELP_MINUTES_MIN} minutes to {} hours, not {text}",
                HELP_MINUTES_MAX / 60
            )
        })
}

/// The minutes in `text`, if it is a length of time at all. A number too
/// long for a count is the largest there is, which no help session lasts.
fn minutes_in(text: &str) -> Option<u64> {
    let number = |digits: &str| {
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(digits.parse().unwrap_or(u64::MAX))
    };
    match text.split_once('h') {
        Some((hours, "")) => Some(number(hours)?.saturating_mul(60)),
        Some((hours, rest)) => {
            let minutes = number(rest.strip_suffix('m')?)?;
            Some(number(hours)?.saturating_mul(60).saturating_add(minutes))
        }
        None => number(text.strip_suffix('m').unwrap_or(text)),
    }
}

/// The key in a public key `line`, as its blob, if it is of a kind sshd
/// takes (ADR 0347).
fn key_from_line(line: &str) -> Result<Vec<u8>, String> {
    let algorithm = line.split_ascii_whitespace().next().unwrap_or_default();
    if !SSH_KEY_ALGORITHMS.contains(&algorithm) {
        let looks_like_a_key =
            ["ssh-", "ecdsa-", "sk-"].iter().any(|start| algorithm.starts_with(start));
        return Err(if looks_like_a_key {
            format!(
                "{algorithm} keys do not sign a helper in: use {}",
                SSH_KEY_ALGORITHMS.join(", ")
            )
        } else {
            "that is not a public key line: give its type, the key and maybe a comment, as in \
             'ssh-ed25519 AAAA... alice@home', quoted as one argument"
                .into()
        });
    }
    sshkeys::public_line(line).ok_or_else(|| format!("the {algorithm} key's text is damaged"))
}

/// The key in the one public key line `path` holds, as `ssh-keygen` writes
/// a `.pub` file.
fn key_from_file(path: &OsStr) -> Result<Vec<u8>, Fail> {
    let shown = Path::new(path).display();
    let text = std::fs::read_to_string(path).map_err(|err| util::failed(&shown, err))?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
    let (Some(line), None) = (lines.next(), lines.next()) else {
        return Err(fail(format!("{shown} must hold one public key line")));
    };
    key_from_line(line).map_err(|why| fail(format!("{shown}: {why}")))
}

/// Starts a help session and shares the terminal until it ends.
fn start(options: &Start) -> Result<i32, Fail> {
    if util::own_user() == 0 {
        return Err(fail("it shares your own screen: run it as yourself, not as root".into()));
    }
    if !util::is_terminal(0) || !util::is_terminal(1) {
        return Err(fail(
            "it shares your terminal, so the standard input and output must both be a terminal"
                .into(),
        ));
    }
    // The help screen has the owner's settings and size, taken before the
    // terminal goes raw.
    let mut screen = Pty::open(0).map_err(|err| util::failed(&"a pseudo-terminal", err))?;
    let (socket, for_sshd) =
        UnixStream::pair().map_err(|err| util::failed(&"a socket pair", err))?;
    let sent: Vec<RawFd> =
        screen.slave.iter().map(AsRawFd::as_raw_fd).chain([for_sshd.as_raw_fd()]).collect();
    let term = std::env::var("TERM").unwrap_or_default();
    let (text, authd) = ask_with_password(
        options.open_ssh,
        |password| Request::HelpStart {
            minutes: options.minutes,
            key: options.key.clone(),
            open_ssh: options.open_ssh,
            term: term.clone(),
            password,
        },
        &sent,
    )?;
    // authd holds them now. Without this one's own copy of the slave side,
    // the master would not report the end of the shell's.
    screen.slave = None;
    drop(for_sshd);
    let started = HelpStarted::from_words(&text)
        .ok_or_else(|| fail(format!("authd answered `ok {text}`")))?;
    show_start(options, &started);
    socket.set_nonblocking(true).map_err(|err| util::failed(&"the helper's socket", err))?;
    let finished = share(&screen.master, &socket, authd).map_err(|fail| Fail {
        message: format!("{}; help-session log {} shows what was kept", fail.message, started.id),
        ..fail
    })?;
    println!("{}", end_message(&started.id, &finished));
    Ok(i32::from(finished.why == HelpEnded::Refused))
}

/// Prints what the owner gives the helper.
fn show_start(options: &Start, started: &HelpStarted) {
    let interfaces = ip::read_interfaces(Path::new(ip::INTERFACES), Path::new(ip::ROUTES))
        .unwrap_or_else(|err| {
            eprintln!(
                "help-session: cannot list this computer's addresses: {}",
                util::strip_os_error(&err)
            );
            Vec::new()
        });
    let until = util::local_time(started.until, "%H:%M");
    let key = options.key.as_deref().map(sshkeys::fingerprint);
    let message = start_message(
        &util::user_name(util::own_user()),
        started,
        options.minutes,
        &until,
        &own_addresses(&interfaces),
        key.as_deref(),
    );
    print!("{message}");
    let _ = io::stdout().flush();
}

/// `minutes` as a person says a length of time, as `1 hour 30 minutes`.
fn length_text(minutes: u32) -> String {
    let plural =
        |count: u32, unit: &str| format!("{count} {unit}{}", if count == 1 { "" } else { "s" });
    match (minutes / 60, minutes % 60) {
        (0, minutes) => plural(minutes, "minute"),
        (hours, 0) => plural(hours, "hour"),
        (hours, minutes) => format!("{} {}", plural(hours, "hour"), plural(minutes, "minute")),
    }
}

/// The addresses a helper can reach this computer at: those of the
/// interfaces that are not the loopback, IPv4 ones, and IPv6 ones that
/// are not link-local, which no route reaches, or still being tested.
fn own_addresses(interfaces: &[Interface]) -> Vec<String> {
    let mut addresses = Vec::new();
    for card in interfaces.iter().filter(|card| !card.loopback) {
        addresses.extend(card.addresses.iter().map(|cidr| cidr.ip.to_string()));
        addresses.extend(
            card.ipv6
                .addresses
                .iter()
                .filter(|address| !address.tentative && !ip::link_local(address.cidr.ip))
                .map(|address| address.cidr.ip.to_string()),
        );
    }
    addresses
}

/// What `help-session start` prints once authd has the session: for
/// `owner`, until the time `until`, `minutes` long, what the helper
/// needs, and `key`, the fingerprint of the key given with `--key`.
fn start_message(
    owner: &str,
    started: &HelpStarted,
    minutes: u32,
    until: &str,
    addresses: &[String],
    key: Option<&str>,
) -> String {
    let mut text = format!(
        "Help session for {owner} until {until} ({}). Give your helper:\n",
        length_text(minutes)
    );
    if addresses.is_empty() {
        text += "  this computer has no network address yet\n";
    }
    for address in addresses {
        text += &format!("  ssh {HELP_NAME}@{address}\n");
    }
    match (&started.code, key) {
        (Some(code), _) => text += &format!("  the code {}\n", help_code_shown(code)),
        (None, Some(key)) => text += &format!("  only the key {key} signs in\n"),
        (None, None) => {}
    }
    match &started.host_key {
        Some(fingerprint) => {
            text += &format!(
                "  this computer's key, which their ssh asks them to check:\n    {fingerprint}\n"
            );
        }
        None => text += "  sshd has no host key yet\n",
    }
    text += "They see and type in this screen, as you. Ctrl-] ends it at any time.\n\
             Everything they type is kept: help-session log shows it.\n\
             Waiting for your helper...\n";
    text
}

/// How a help session ended, and whether a helper signed in before.
struct Finished {
    why: HelpEnded,
    helper_came: bool,
}

/// What `help-session start` prints at the end of session `id`.
fn end_message(id: &str, finished: &Finished) -> String {
    let mut text = format!(
        "Help session {id} ended: {}. What your helper typed is kept: help-session log {id}.",
        ended_why(finished.why)
    );
    if finished.helper_came {
        text += "\nWhat your helper changed in your files and settings stays, as your own changes \
                 would.";
    }
    text
}

/// Why a session ended, in words that read right on the owner's screen
/// and the helper's.
fn ended_why(why: HelpEnded) -> &'static str {
    match why {
        HelpEnded::Owner => "the owner ended it",
        HelpEnded::Deadline => "the time was up",
        HelpEnded::Helper => "the helper left",
        HelpEnded::Session => "the shell ended",
        HelpEnded::Spent => "the code was spent",
        HelpEnded::Refused => "the start was refused",
    }
}

/// What authd's `event` says, as a sentence for the owner's screen and,
/// for the events the helper is told, theirs.
fn event_sentence(event: &HelpEvent) -> String {
    match event {
        HelpEvent::Helper { address, key } if key == "code" => {
            format!("Your helper signed in from {address} with the code.")
        }
        HelpEvent::Helper { address, key } => {
            format!("Your helper signed in from {address} with the key {key}.")
        }
        HelpEvent::Left { minutes } => {
            format!(
                "{minutes} minute{} left in this help session.",
                if *minutes == 1 { "" } else { "s" }
            )
        }
        HelpEvent::Elevate { minutes } => {
            format!("elevate runs in the help screen for {minutes} minutes.")
        }
        HelpEvent::Spent => {
            "Five wrong codes spent the code: start a new help session to give another.".into()
        }
        HelpEvent::Ended(why) => format!("This help session ended: {}.", ended_why(*why)),
    }
}

/// `sentence` on lines of its own, marked as help-session's so that it
/// reads apart from the screen. The terminals are raw, which has no
/// carriage return after a line feed on its own.
fn framed(sentence: &str) -> String {
    format!("\r\n[help-session] {sentence}\r\n")
}

/// What the owner typed in `bytes` that goes to the help screen, and
/// whether Ctrl-] came in it, which ends the session; what they typed
/// after it is dropped.
fn before_end_key(bytes: &[u8]) -> (&[u8], bool) {
    match bytes.iter().position(|&byte| byte == END_KEY) {
        Some(at) => (&bytes[..at], true),
        None => (bytes, false),
    }
}

/// Sends authd the request that `request` builds from a password typed
/// here (none, unless `needs_password`) with the descriptors `fds`: its
/// answer, and its connection, which stays open for what it says later.
/// A wrong password is asked for again, up to [`TRIES`] times.
fn ask_with_password(
    needs_password: bool,
    request: impl Fn(Vec<u8>) -> Request,
    fds: &[RawFd],
) -> Result<(String, BufReader<UnixStream>), Fail> {
    let mut wrong = 0;
    loop {
        let password = if needs_password { password_typed()? } else { Vec::new() };
        match elevate::ask_with(&request(password), fds)? {
            (Answer::Done(text), connection) => return Ok((text, connection)),
            (Answer::Refused(why), _) if why == WRONG_PASSWORD => {
                wrong += 1;
                if wrong >= TRIES {
                    return Err(fail(format!("{TRIES} wrong passwords")));
                }
                eprintln!("Sorry, try again.");
            }
            (Answer::Refused(why), _) => return Err(fail(why)),
        }
    }
}

/// The owner's password, asked at the terminal with its echo off.
fn password_typed() -> Result<Vec<u8>, Fail> {
    let question = format!("[help-session] password for {}: ", util::user_name(util::own_user()));
    match prompt::ask_on(0, OWNER_OUT, &question, false) {
        Ok(prompt::Answer::Line(line)) => Ok(line),
        Ok(prompt::Answer::Stopped) => Err(fail("no password was typed".into())),
        Err(err) => Err(fail(format!("cannot read the password: {}", util::strip_os_error(&err)))),
    }
}

/// Joins the help screen to the owner's terminal and the helper's socket
/// until the session ends: the terminal is raw meanwhile, and put back
/// whatever ends the loop. Dropping `authd` at the end closes the
/// connection, which is what ends a session its owner ended.
fn share(
    master: &OwnedFd,
    socket: &UnixStream,
    mut authd: BufReader<UnixStream>,
) -> Result<Finished, Fail> {
    let (wake, wake_write) = elevate::pipe().map_err(|err| util::failed(&"a pipe", err))?;
    elevate::WAKE.store(wake_write.as_raw_fd(), Ordering::Relaxed);
    // SAFETY: the handler only writes to the pipe; the signals' old
    // actions are not needed again, as help-session ends after.
    unsafe {
        let mut action: libc::sigaction = util::zeroed();
        action.sa_sigaction =
            elevate::on_signal as extern "C" fn(libc::c_int) as *const () as usize;
        action.sa_flags = libc::SA_RESTART;
        for signal in [libc::SIGWINCH, libc::SIGTERM, libc::SIGHUP, libc::SIGINT, libc::SIGQUIT] {
            libc::sigaction(signal, &raw const action, std::ptr::null_mut());
        }
        // A helper whose connection went must not end help-session with
        // the terminal still raw.
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let mut raw = Raw::new(0);
    raw.on();
    let mut joined = Joined {
        master,
        socket,
        master_open: true,
        socket_open: true,
        helper_came: false,
        to_helper: Vec::new(),
        buffer: [0; 4096],
    };
    let why = joined.run(&mut authd, &wake);
    raw.off();
    why.map(|why| Finished { why, helper_came: joined.helper_came })
}

/// The sources [`share`] reads, and what it has seen.
struct Joined<'a> {
    master: &'a OwnedFd,
    socket: &'a UnixStream,
    /// Whether the help screen and the helper's socket can still be read:
    /// each is let alone once it reports its end.
    master_open: bool,
    socket_open: bool,
    /// Whether a helper signed in: the screen goes to them from then on.
    helper_came: bool,
    /// What the helper's socket has not taken yet.
    to_helper: Vec<u8>,
    buffer: [u8; 4096],
}

impl Joined<'_> {
    /// Copies between the owner, the help screen, the helper and authd's
    /// events until the session ends: why.
    fn run(
        &mut self,
        authd: &mut BufReader<UnixStream>,
        wake: &OwnedFd,
    ) -> Result<HelpEnded, Fail> {
        let watch = |fd: RawFd, events: libc::c_short| libc::pollfd {
            fd: if events == 0 { -1 } else { fd },
            events,
            revents: 0,
        };
        let when = |open: bool, events: libc::c_short| if open { events } else { 0 };
        loop {
            let to_socket = when(self.socket_open, libc::POLLIN)
                | when(!self.to_helper.is_empty(), libc::POLLOUT);
            let mut fds = [
                watch(authd.get_ref().as_raw_fd(), libc::POLLIN),
                watch(wake.as_raw_fd(), libc::POLLIN),
                watch(self.master.as_raw_fd(), when(self.master_open, libc::POLLIN)),
                watch(0, libc::POLLIN),
                watch(self.socket.as_raw_fd(), to_socket),
            ];
            // A line of authd's already read is taken before waiting.
            let buffered = !authd.buffer().is_empty();
            // SAFETY: `fds` holds valid pollfds.
            let ready = unsafe {
                libc::poll(
                    fds.as_mut_ptr(),
                    fds.len() as libc::nfds_t,
                    if buffered { 0 } else { -1 },
                )
            };
            if ready < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(util::failed(&"poll", err));
            }
            if fds[1].revents != 0 && self.signals(wake) {
                return Ok(HelpEnded::Owner);
            }
            if fds[3].revents != 0 && self.owner_typed() {
                return Ok(HelpEnded::Owner);
            }
            if fds[4].revents & libc::POLLOUT != 0 {
                self.send_helper();
            }
            if self.socket_open && fds[4].revents & !libc::POLLOUT != 0 {
                self.helper_typed();
            }
            if fds[2].revents != 0 {
                self.screen_wrote();
            }
            if (fds[0].revents != 0 || buffered)
                && let Some(why) = self.authd_said(authd)?
            {
                return Ok(why);
            }
        }
    }

    /// Takes the signals that came: the terminal's new size goes to the
    /// help screen. Whether one asks to end the session.
    fn signals(&self, wake: &OwnedFd) -> bool {
        let mut signals = [0u8; 16];
        let count = elevate::read_some(wake.as_raw_fd(), &mut signals);
        for &signal in &signals[..count] {
            if libc::c_int::from(signal) != libc::SIGWINCH {
                return true;
            }
            self.copy_size();
        }
        false
    }

    /// Gives the help screen the size of the owner's terminal: the
    /// helper's own window is ignored.
    fn copy_size(&self) {
        // SAFETY: all zeros is a valid winsize: numbers.
        let mut size: libc::winsize = unsafe { util::zeroed() };
        // SAFETY: TIOCGWINSZ fills `size`, which TIOCSWINSZ reads.
        unsafe {
            if libc::ioctl(0, libc::TIOCGWINSZ, &raw mut size) == 0 {
                libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &raw const size);
            }
        }
    }

    /// Passes what the owner typed to the help screen. Whether it ends
    /// the session: Ctrl-], or the terminal's hangup.
    fn owner_typed(&mut self) -> bool {
        let count = elevate::read_some(0, &mut self.buffer);
        if count == 0 {
            return true;
        }
        let (typed, ends) = before_end_key(&self.buffer[..count]);
        elevate::write_all(self.master.as_raw_fd(), typed);
        ends
    }

    /// Passes what the helper typed to the help screen. Ctrl-] from them
    /// is a key like any other: only the owner's ends the session.
    fn helper_typed(&mut self) {
        let mut socket = self.socket;
        match socket.read(&mut self.buffer) {
            Ok(0) => self.socket_open = false,
            Ok(count) => elevate::write_all(self.master.as_raw_fd(), &self.buffer[..count]),
            // Nothing to read yet is no end; a reset connection is as ended
            // as a closed one.
            Err(err) => {
                self.socket_open =
                    matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted);
            }
        }
    }

    /// Shows what the help screen wrote to the owner and the helper.
    fn screen_wrote(&mut self) {
        let count = elevate::read_some(self.master.as_raw_fd(), &mut self.buffer);
        if count == 0 {
            // Every descriptor of the help screen's terminal is closed.
            self.master_open = false;
            return;
        }
        elevate::write_all(OWNER_OUT, &self.buffer[..count]);
        let output = self.buffer;
        self.tell_helper(&output[..count]);
    }

    /// Sends `bytes` to the helper, once one has signed in: what the
    /// socket does not take now waits for it, up to [`BEHIND_MAX`].
    fn tell_helper(&mut self, bytes: &[u8]) {
        if !self.helper_came {
            return;
        }
        if self.to_helper.len() + bytes.len() > BEHIND_MAX {
            self.to_helper.clear();
            self.to_helper.extend_from_slice(framed(FELL_BEHIND).as_bytes());
        }
        self.to_helper.extend_from_slice(bytes);
        self.send_helper();
    }

    /// Sends the helper as much of what waits for them as the socket takes
    /// now, which never blocks. A socket that takes nothing more, as when
    /// the helper's connection is gone, has what waits dropped.
    fn send_helper(&mut self) {
        let mut socket = self.socket;
        while !self.to_helper.is_empty() {
            match socket.write(&self.to_helper) {
                Ok(0) => return self.to_helper.clear(),
                Ok(count) => drop(self.to_helper.drain(..count)),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return,
                Err(_) => return self.to_helper.clear(),
            }
        }
    }

    /// Takes one line of authd's events and shows it. Why the session
    /// ended, when the line says so.
    fn authd_said(&mut self, authd: &mut BufReader<UnixStream>) -> Result<Option<HelpEnded>, Fail> {
        let mut line = String::new();
        match authd.read_line(&mut line) {
            Ok(0) | Err(_) => {
                return Err(fail(
                    "authd closed its connection before the help session ended".into(),
                ));
            }
            Ok(_) => {}
        }
        let Some(event) = HelpEvent::parse(&line) else {
            elevate::write_all(
                OWNER_OUT,
                framed(&format!("authd said `{}`.", line.trim_end())).as_bytes(),
            );
            return Ok(None);
        };
        let text = framed(&event_sentence(&event));
        match event {
            // The owner's screen says it after the terminal is put back;
            // sshd tells the helper, as authd has closed their socket.
            HelpEvent::Ended(why) => return Ok(Some(why)),
            HelpEvent::Helper { .. } => self.helper_came = true,
            HelpEvent::Left { .. } => self.tell_helper(text.as_bytes()),
            _ => {}
        }
        elevate::write_all(OWNER_OUT, text.as_bytes());
        Ok(None)
    }
}

/// `help-session end`: ends the owner's session from another terminal.
fn end() -> Result<i32, Fail> {
    elevate::done(account::ask(&Request::HelpEnd)?)?;
    println!("Ended your help session.");
    Ok(0)
}

/// `help-session log`: the owner's kept sessions, or one's transcript.
fn log(id: Option<&str>) -> Result<i32, Fail> {
    let request = Request::HelpLog { id: id.map(String::from) };
    let words = elevate::done(account::ask(&request)?)?;
    let lines: Vec<&str> = words.split('\t').filter(|line| !line.is_empty()).collect();
    let text = if id.is_some() {
        transcript_text(&lines, |seconds| util::local_time(seconds, "%H:%M:%S"))
    } else {
        kept_text(&lines, |seconds| util::local_time(seconds, "%Y-%m-%d %H:%M"))
    };
    print!("{text}");
    Ok(0)
}

/// The list of kept sessions, `lines` as authd gave them, as a table with
/// the times as `when` shows them.
fn kept_text(lines: &[&str], when: impl Fn(i64) -> String) -> String {
    let kept: Vec<HelpKept> = lines.iter().copied().filter_map(HelpKept::from_words).collect();
    if kept.is_empty() {
        return "No help session is kept.\n".into();
    }
    let rows: Vec<[String; 4]> = kept
        .iter()
        .map(|kept| {
            let ended = if kept.ended == 0 { "running".into() } else { when(kept.ended) };
            [kept.id.clone(), kept.owner.clone(), when(kept.started), ended]
        })
        .collect();
    let header = ["ID", "Owner", "Started", "Ended"].map(String::from);
    let widths: [usize; 3] = std::array::from_fn(|column| {
        rows.iter().fold(header[column].len(), |most, row| most.max(row[column].len()))
    });
    let mut text = String::new();
    for row in [&header].into_iter().chain(&rows) {
        text += &format!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {}\n",
            row[0],
            row[1],
            row[2],
            row[3],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2]
        );
    }
    text
}

/// A transcript's `lines`, each `SECONDS KIND TEXT`, as a person reads
/// them: the time as `clock` shows it, then the kind and the text. A line
/// that is not in that form is shown as it is.
fn transcript_text(lines: &[&str], clock: impl Fn(i64) -> String) -> String {
    let mut text = String::new();
    for line in lines {
        let mut words = line.splitn(3, ' ');
        match (words.next().and_then(|seconds| seconds.parse().ok()), words.next(), words.next()) {
            (Some(seconds), Some(kind), typed) => {
                text += &format!("{}  {kind:<7} {}\n", clock(seconds), typed.unwrap_or_default());
            }
            _ => text += &format!("{line}\n"),
        }
    }
    text
}

/// `help-session allow-elevate`: lets elevate run in the help screen
/// without a password, after the owner's, asked here.
fn allow_elevate() -> Result<i32, Fail> {
    if !util::is_terminal(0) {
        return Err(fail("help-session allow-elevate asks for your password at a terminal".into()));
    }
    let (text, _) =
        ask_with_password(true, |password| Request::HelpAllowElevate { password }, &[])?;
    let minutes: u32 = text.parse().map_err(|_| fail(format!("authd answered `ok {text}`")))?;
    println!("elevate runs in your help screen for {minutes} minutes, without a password.");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::ip::{Address6, Cidr, Cidr6, Ipv6, Kind, Origin};
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    /// An Ed25519 key of the points 0 to 31.
    const ED25519: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";

    fn started(code: Option<&str>, host_key: Option<&str>) -> HelpStarted {
        HelpStarted {
            id: "k7q2mx".into(),
            until: 0,
            code: code.map(String::from),
            host_key: host_key.map(String::from),
        }
    }

    #[test]
    fn lengths_are_read_as_people_write_them() {
        for (text, minutes) in [
            ("90m", 90),
            ("2h", 120),
            ("1h30m", 90),
            ("45", 45),
            ("5", 5),
            ("5m", 5),
            ("480", 480),
            ("8h", 480),
            ("0h45m", 45),
            ("7h60m", 480),
        ] {
            assert_eq!(parse_minutes(text), Ok(minutes), "{text}");
        }
    }

    #[test]
    fn lengths_outside_five_minutes_to_eight_hours_are_refused_with_the_range() {
        for text in ["4", "0", "481", "9h", "8h1m", "4m", "99999999999999999999", "99999999999h"] {
            let why = parse_minutes(text).unwrap_err();
            assert_eq!(why, format!("a help session lasts 5 minutes to 8 hours, not {text}"));
        }
    }

    #[test]
    fn what_is_no_length_is_refused_with_how_to_write_one() {
        for text in
            ["", "h", "m", "1.5h", "1m30h", "1h30", "abc", "-5", "1h 30m", "1h30m5", "+5", "2H"]
        {
            let why = parse_minutes(text).unwrap_err();
            assert!(why.contains("is not a length of time"), "{text}: {why}");
        }
    }

    #[test]
    fn a_public_key_line_gives_its_blob() {
        let blob = STANDARD.decode(ED25519).unwrap();
        for line in [
            format!("ssh-ed25519 {ED25519}"),
            format!("  ssh-ed25519   {ED25519}   alice@laptop with a comment"),
        ] {
            assert_eq!(key_from_line(&line), Ok(blob.clone()), "{line}");
        }
    }

    #[test]
    fn a_key_of_a_kind_sshd_does_not_take_is_refused_by_name() {
        let mut rsa = vec![0, 0, 0, 7];
        rsa.extend_from_slice(b"ssh-rsa");
        let why = key_from_line(&format!("ssh-rsa {}", STANDARD.encode(rsa))).unwrap_err();
        assert!(
            why.starts_with("ssh-rsa keys do not sign a helper in: use ssh-ed25519, "),
            "{why}"
        );
        assert!(
            key_from_line("ecdsa-sha2-nistp521 AAAA")
                .unwrap_err()
                .contains("ecdsa-sha2-nistp521 keys")
        );
        assert!(
            key_from_line("sk-ssh-ed25519@openssh.com AAAA")
                .unwrap_err()
                .contains("sk-ssh-ed25519@openssh.com keys")
        );
    }

    #[test]
    fn what_is_no_key_line_is_refused_with_an_example() {
        for line in ["", "hello", "alice@laptop", "command=\"x\" ssh-ed25519 AAAA"] {
            let why = key_from_line(line).unwrap_err();
            assert!(why.starts_with("that is not a public key line"), "{line}: {why}");
        }
        // The type is right and the key is not.
        assert_eq!(
            key_from_line("ssh-ed25519").unwrap_err(),
            "the ssh-ed25519 key's text is damaged"
        );
        let other = format!("ecdsa-sha2-nistp256 {ED25519}");
        assert_eq!(
            key_from_line(&other).unwrap_err(),
            "the ecdsa-sha2-nistp256 key's text is damaged"
        );
    }

    #[test]
    fn a_key_file_holds_one_line_among_its_blanks_and_comments() {
        let dir = std::env::temp_dir().join(format!("help-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, text: &str| {
            let path = dir.join(name);
            std::fs::write(&path, text).unwrap();
            path.into_os_string()
        };
        let one = file("one.pub", &format!("# alice\n\nssh-ed25519 {ED25519} alice\n"));
        assert_eq!(key_from_file(&one).unwrap(), STANDARD.decode(ED25519).unwrap());
        let two = file("two.pub", &format!("ssh-ed25519 {ED25519}\nssh-ed25519 {ED25519}\n"));
        assert!(
            key_from_file(&two).unwrap_err().message.ends_with("must hold one public key line")
        );
        let none = file("none.pub", "# nothing\n");
        assert!(key_from_file(&none).is_err());
        let bad = file("bad.pub", "hello\n");
        assert!(
            key_from_file(&bad)
                .unwrap_err()
                .message
                .contains("bad.pub: that is not a public key line")
        );
        assert!(
            key_from_file(&OsString::from(dir.join("missing.pub")))
                .unwrap_err()
                .message
                .contains("missing.pub")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lengths_are_said_in_hours_and_minutes() {
        for (minutes, text) in [
            (5, "5 minutes"),
            (45, "45 minutes"),
            (60, "1 hour"),
            (90, "1 hour 30 minutes"),
            (121, "2 hours 1 minute"),
            (480, "8 hours"),
        ] {
            assert_eq!(length_text(minutes), text);
        }
    }

    #[test]
    fn the_start_message_gives_the_address_the_code_and_the_host_key() {
        let fingerprint = "SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f";
        let message = start_message(
            "alice",
            &started(Some("KQ7RM2XW9HTP4BZE"), Some(fingerprint)),
            60,
            "15:04",
            &["192.168.1.20".into(), "2001:db8::5".into()],
            None,
        );
        assert_eq!(
            message,
            "Help session for alice until 15:04 (1 hour). Give your helper:\n\
             \x20 ssh help@192.168.1.20\n\
             \x20 ssh help@2001:db8::5\n\
             \x20 the code KQ7R-M2XW-9HTP-4BZE\n\
             \x20 this computer's key, which their ssh asks them to check:\n\
             \x20   SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f\n\
             They see and type in this screen, as you. Ctrl-] ends it at any time.\n\
             Everything they type is kept: help-session log shows it.\n\
             Waiting for your helper...\n"
        );
    }

    #[test]
    fn with_a_key_only_it_signs_in_and_no_code_is_given() {
        let message = start_message(
            "bob",
            &started(None, None),
            90,
            "09:30",
            &["10.0.0.7".into()],
            Some("SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA"),
        );
        assert!(message.starts_with("Help session for bob until 09:30 (1 hour 30 minutes). Give"));
        assert!(message.contains(
            "  only the key SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA signs in\n"
        ));
        assert!(!message.contains("the code"));
        assert!(message.contains("  sshd has no host key yet\n"));
        assert!(!message.contains("which their ssh asks them to check"));
    }

    #[test]
    fn a_computer_with_no_address_says_so_instead_of_an_ssh_line() {
        let message =
            start_message("alice", &started(Some("KQ7RM2XW9HTP4BZE"), None), 5, "10:00", &[], None);
        assert!(message.contains("  this computer has no network address yet\n"));
        assert!(!message.contains("ssh help@"));
    }

    fn card(name: &str, loopback: bool, v4: &[&str], v6: &[(&str, bool)]) -> Interface {
        Interface {
            name: name.into(),
            index: 1,
            mtu: 1500,
            mac: String::new(),
            loopback,
            carrier: true,
            operstate: "up".into(),
            addresses: v4.iter().map(|text| Cidr::parse(text).unwrap()).collect(),
            router: None,
            ipv6: Ipv6 {
                addresses: v6
                    .iter()
                    .map(|(text, tentative)| Address6 {
                        cidr: Cidr6::parse(text).unwrap(),
                        origin: Origin::Given,
                        preferred: None,
                        valid: None,
                        tentative: *tentative,
                    })
                    .collect(),
                ..Ipv6::default()
            },
            kind: Kind::Card,
            master: None,
            received: [0; 4],
            sent: [0; 4],
        }
    }

    #[test]
    fn the_addresses_given_are_those_a_helper_can_reach() {
        let cards = [
            card("lo", true, &["127.0.0.1/8"], &[("::1/128", false)]),
            card(
                "net0",
                false,
                &["192.168.1.20/24"],
                &[("fe80::1/64", false), ("2001:db8::5/64", false), ("2001:db8::6/64", true)],
            ),
            card("net1", false, &["10.0.0.7/8"], &[]),
        ];
        assert_eq!(own_addresses(&cards), ["192.168.1.20", "2001:db8::5", "10.0.0.7"]);
    }

    #[test]
    fn events_are_told_in_sentences() {
        let said = |line: &str| event_sentence(&HelpEvent::parse(line).unwrap());
        assert_eq!(
            said("helper 192.0.2.7 code"),
            "Your helper signed in from 192.0.2.7 with the code."
        );
        assert_eq!(
            said("helper 192.0.2.7 SHA256:abc"),
            "Your helper signed in from 192.0.2.7 with the key SHA256:abc."
        );
        assert_eq!(said("left 5"), "5 minutes left in this help session.");
        assert_eq!(said("left 1"), "1 minute left in this help session.");
        assert_eq!(said("elevate 15"), "elevate runs in the help screen for 15 minutes.");
        assert_eq!(
            said("spent"),
            "Five wrong codes spent the code: start a new help session to give another."
        );
        assert_eq!(said("ended deadline"), "This help session ended: the time was up.");
    }

    #[test]
    fn every_end_has_its_words() {
        let whys = [
            (HelpEnded::Owner, "the owner ended it"),
            (HelpEnded::Deadline, "the time was up"),
            (HelpEnded::Helper, "the helper left"),
            (HelpEnded::Session, "the shell ended"),
            (HelpEnded::Spent, "the code was spent"),
            (HelpEnded::Refused, "the start was refused"),
        ];
        for (why, words) in whys {
            assert_eq!(ended_why(why), words);
        }
    }

    #[test]
    fn a_framed_line_stands_on_lines_of_its_own_for_a_raw_terminal() {
        assert_eq!(
            framed("Your helper signed in from 192.0.2.7 with the code."),
            "\r\n[help-session] Your helper signed in from 192.0.2.7 with the code.\r\n"
        );
    }

    #[test]
    fn the_end_names_the_transcript_and_what_stays_when_a_helper_came() {
        let alone = Finished { why: HelpEnded::Owner, helper_came: false };
        assert_eq!(
            end_message("k7q2mx", &alone),
            "Help session k7q2mx ended: the owner ended it. What your helper typed is kept: \
             help-session log k7q2mx."
        );
        let helped = Finished { why: HelpEnded::Helper, helper_came: true };
        assert_eq!(
            end_message("k7q2mx", &helped),
            "Help session k7q2mx ended: the helper left. What your helper typed is kept: \
             help-session log k7q2mx.\nWhat your helper changed in your files and settings \
             stays, as your own changes would."
        );
    }

    #[test]
    fn ctrl_right_bracket_ends_the_session_and_what_follows_it_is_dropped() {
        assert_eq!(before_end_key(b"ls -l\r"), (&b"ls -l\r"[..], false));
        assert_eq!(before_end_key(b"\x1d"), (&b""[..], true));
        assert_eq!(before_end_key(b"ab\x1dcd\x1d"), (&b"ab"[..], true));
        assert_eq!(before_end_key(b""), (&b""[..], false));
    }

    #[test]
    fn the_list_of_kept_sessions_is_a_table_with_local_times() {
        let when = |seconds: i64| format!("t{seconds}");
        let lines = ["k7q2mx alice 100 200", "zz9 bob 300 0"];
        assert_eq!(
            kept_text(&lines, when),
            "ID      Owner  Started  Ended\n\
             k7q2mx  alice  t100     t200\n\
             zz9     bob    t300     running\n"
        );
        assert_eq!(kept_text(&[], when), "No help session is kept.\n");
    }

    #[test]
    fn a_transcript_shows_each_line_with_its_time() {
        let clock = |seconds: i64| format!("t{seconds}");
        let lines = [
            "100 start alice started it, until 15:04, for a helper with a code",
            "130 helper signed in from 192.0.2.7 with the code",
            "140 typed echo helped > ~/proof",
            "150 typed ",
            "160 end owner",
            "odd line",
        ];
        assert_eq!(
            transcript_text(&lines, clock),
            "t100  start   alice started it, until 15:04, for a helper with a code\n\
             t130  helper  signed in from 192.0.2.7 with the code\n\
             t140  typed   echo helped > ~/proof\n\
             t150  typed   \n\
             t160  end     owner\n\
             odd line\n"
        );
    }

    #[test]
    fn the_helper_sees_the_screen_from_their_sign_in_and_a_slow_one_is_told_what_was_skipped() {
        let (socket, helper) = UnixStream::pair().unwrap();
        socket.set_nonblocking(true).unwrap();
        helper.set_nonblocking(true).unwrap();
        let master = OwnedFd::from(std::fs::File::open("/dev/null").unwrap());
        let mut joined = Joined {
            master: &master,
            socket: &socket,
            master_open: true,
            socket_open: true,
            helper_came: false,
            to_helper: Vec::new(),
            buffer: [0; 4096],
        };
        let read = |helper: &UnixStream| {
            let mut got = Vec::new();
            let mut buffer = [0; 16384];
            let mut helper = helper;
            while let Ok(count) = helper.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                got.extend_from_slice(&buffer[..count]);
            }
            got
        };
        // Before a helper signs in, nothing goes to the socket.
        joined.tell_helper(b"before");
        assert!(read(&helper).is_empty());
        joined.helper_came = true;
        joined.tell_helper(b"after");
        assert_eq!(read(&helper), b"after");
        // A helper who reads nothing: the socket fills, and what it cannot
        // take waits, up to the most kept; then that is dropped for a note.
        let chunk = [b'x'; 4096];
        while joined.to_helper.is_empty() {
            joined.tell_helper(&chunk);
        }
        while joined.to_helper.len() + chunk.len() <= BEHIND_MAX {
            joined.tell_helper(&chunk);
        }
        joined.tell_helper(b"latest");
        let note = framed(FELL_BEHIND);
        assert!(joined.to_helper.starts_with(note.as_bytes()));
        assert!(joined.to_helper.ends_with(b"latest"));
        // As the helper reads, the rest goes, the note among it.
        let mut got = Vec::new();
        while !joined.to_helper.is_empty() {
            got.extend(read(&helper));
            joined.send_helper();
        }
        got.extend(read(&helper));
        let text = String::from_utf8_lossy(&got);
        assert!(text.contains(FELL_BEHIND) && text.ends_with("latest"));
        // A helper who is gone takes nothing more, and nothing piles up.
        drop(helper);
        joined.tell_helper(b"to no one");
        assert!(joined.to_helper.is_empty());
    }

    #[test]
    fn the_usage_names_the_range_of_lengths() {
        let text = usage_text();
        assert!(text.starts_with("usage: help-session start [--for DURATION]"));
        assert!(text.contains("5 minutes to 8 hours, 60 minutes without --for."));
    }
}
