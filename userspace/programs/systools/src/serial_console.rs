// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! serial-console(8): a console on the PC's first serial port,
//! `/devices/ttyS0`, for a computer with no screen (Phase 14, ADR 0318),
//! as agetty(8) keeps one on Linux.
//!
//! Spark starts it at every boot. It does something only when the kernel
//! was started with `console=ttyS0`, as the install stick's "Install
//! Dukovo (serial console)" entry and a system installed from it are
//! (dukovo-boot(7)); otherwise it ends at once and stays stopped. Then it
//! runs, on the line, and again each time it ends:
//!
//! - under the kernel flag `install`, the installer's screens, once, as
//!   consoled(8) runs them on tty1, which then leaves them to the line;
//! - under `console-sign-in=off`, root's shell, with no sign-in;
//! - otherwise the issue text, /system/settings/issue, and the sign-in,
//!   login(1).
//!
//! Each session finds the line as a new terminal is, at the speed and
//! character format `console=ttyS0,115200n8` gives (115200n8 when it
//! gives none, the line GRUB and the kernel log run), whatever the last
//! session left; with TERM=vt220, as Linux's serial consoles have; and
//! with the size of the terminal at the other end, which is asked for as
//! xterm's resize(1) asks, or 80 by 24 when nothing answers.
//!
//! serial-console holds no descriptor of the line while a session runs,
//! so that once the session's programs are done with it, the kernel's
//! log goes out on it again until the next session starts.

use std::ffi::CString;
use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::args::Args;
use crate::elevate;
use crate::util::{self, Fail};

/// The PC's first serial port, COM1, as a terminal.
const LINE: &str = "/devices/ttyS0";

/// The line's name, as the issue text's `\l` and the sign-in records give
/// it.
const LINE_NAME: &str = "ttyS0";

/// The kernel flag that asks for the console, with Linux's options after
/// a comma: `console=ttyS0,115200n8`.
const FLAG: &str = "console=ttyS0";

/// What runs once under the kernel flag `install`.
const INSTALLER: &str = "/system/commands/dukovo-install";

/// What signs people in.
const LOGIN: &str = "/system/commands/login";

/// The text shown before the sign-in and root's shell, as on the screen's
/// consoles.
const ISSUE: &str = "/system/settings/issue";

/// What is shown when /system/settings/issue is missing, as consoled(8)
/// shows.
const DEFAULT_ISSUE: &str = "\\s \\r (\\l)\n\n";

/// The terminal type a serial console's programs are told, as systemd's
/// serial consoles tell theirs: what nearly every terminal and terminal
/// program at the other end of a line understands.
const TERM: &str = "vt220";

/// Rows and columns when the terminal at the other end does not say.
const FALLBACK_SIZE: (u16, u16) = (24, 80);

/// The question xterm's resize(1) asks a terminal: keep the cursor, send
/// it as far down and right as it goes, report where it is, and put it
/// back. The report is `ESC [ rows ; columns R`.
const ASK_SIZE: &[u8] = b"\x1b7\x1b[999;999H\x1b[6n\x1b8";

/// How long the answer may take: a line at 115200 baud carries it in a
/// millisecond, and a terminal program answers at once.
const ANSWER_WAIT: Duration = Duration::from_millis(500);

/// A size smaller than this is no answer: a terminal that did not move
/// the cursor reports 1;1.
const SMALLEST: (u16, u16) = (5, 20);

/// A session that ends sooner than this after it started ended at once.
const QUICK: Duration = Duration::from_secs(2);

/// After this many sessions in a row ended at once, serial-console waits
/// before it starts another, as consoled(8) does.
const QUICK_MAX: u32 = 5;

/// How long it waits then.
const PAUSE: Duration = Duration::from_secs(10);

/// How `console=ttyS0,…` asks the line to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Format {
    /// The speed, as its termios code: `B115200`.
    speed: libc::speed_t,
    /// The parity's control flags: none, `PARENB`, or `PARENB | PARODD`.
    parity: libc::tcflag_t,
    /// The character size's: `CS7` or `CS8`.
    size: libc::tcflag_t,
}

impl Format {
    /// 115200n8: what GRUB and the kernel's log run the line at.
    const DEFAULT: Format = Format { speed: libc::B115200, parity: 0, size: libc::CS8 };
}

/// The speeds the PC's serial port runs at, in baud, with their codes:
/// a 16550 divides 115200 baud by a whole number.
const SPEEDS: &[(u32, libc::speed_t)] = &[
    (300, libc::B300),
    (600, libc::B600),
    (1200, libc::B1200),
    (2400, libc::B2400),
    (4800, libc::B4800),
    (9600, libc::B9600),
    (19200, libc::B19200),
    (38400, libc::B38400),
    (57600, libc::B57600),
    (115_200, libc::B115200),
];

/// What the kernel flags of command line `line`, those before a lone
/// `--`, ask of the serial console: nothing without `console=ttyS0`;
/// otherwise its format, or why its options are not one. Of several, the
/// last counts, as on Linux.
fn asked(line: &str) -> Option<Result<Format, String>> {
    let flag = line
        .split_whitespace()
        .take_while(|word| *word != "--")
        .filter(|word| word.strip_prefix(FLAG).is_some_and(|r| r.is_empty() || r.starts_with(',')))
        .last()?;
    Some(match flag.strip_prefix(FLAG).and_then(|rest| rest.strip_prefix(',')) {
        None => Ok(Format::DEFAULT),
        Some(options) => format_of(options),
    })
}

/// The format Linux's console options `options` give: the speed in baud,
/// then optionally the parity, `n`, `o` or `e`, and the data bits, `7` or
/// `8`, as in `115200n8`. A final `r`, RTS/CTS flow control, is passed
/// over: the PC's serial port does none (ADR 0318).
fn format_of(options: &str) -> Result<Format, String> {
    let digits = options.find(|c: char| !c.is_ascii_digit()).unwrap_or(options.len());
    let (speed, rest) = options.split_at(digits);
    let speed = speed
        .parse::<u32>()
        .ok()
        .and_then(|baud| SPEEDS.iter().find(|(speed, _)| *speed == baud))
        .map(|(_, code)| *code)
        .ok_or_else(|| {
            format!("{FLAG},{options}: the speed must be one of 300, 600, 1200, ... 57600, 115200")
        })?;
    let mut rest = rest.chars();
    let parity = match rest.next() {
        None | Some('n') => 0,
        Some('o') => libc::PARENB | libc::PARODD,
        Some('e') => libc::PARENB,
        Some(other) => {
            return Err(format!("{FLAG},{options}: the parity must be n, o or e, not {other}"));
        }
    };
    let size = match rest.next() {
        None | Some('8') => libc::CS8,
        Some('7') => libc::CS7,
        Some(other) => {
            return Err(format!("{FLAG},{options}: the data bits must be 7 or 8, not {other}"));
        }
    };
    // Only flow control may follow, which is passed over.
    let flow: String = rest.collect();
    if !flow.is_empty() && flow != "r" {
        return Err(format!("{FLAG},{options}: more than a speed, a parity, data bits and r"));
    }
    Ok(Format { speed, parity, size })
}

/// What a session runs.
struct Session {
    /// The program's path.
    path: String,
    /// Its name, as its argv[0]: a login shell's starts with a dash.
    name: String,
    /// Whether it is login(1), which signs people in: it gets the line
    /// but not as its controlling terminal, which the session it starts
    /// takes (ADR 0250), as consoled(8) runs it. Anything else is root's
    /// and has the line as its own.
    sign_in: bool,
    /// Whether the issue text shows first: not before the installer,
    /// which draws the whole screen.
    issue: bool,
}

impl Session {
    /// A program run once, as the installer: by its own name.
    fn once(path: &str) -> Session {
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        Session { path: path.to_string(), name, sign_in: false, issue: false }
    }

    /// Root's shell, `shell`, as a login shell.
    fn shell(shell: &str) -> Session {
        let name = format!("-{}", shell.rsplit('/').next().unwrap_or("sh"));
        Session { path: shell.to_string(), name, sign_in: false, issue: true }
    }

    /// The sign-in.
    fn sign_in() -> Session {
        Session { path: LOGIN.to_string(), name: "login".to_string(), sign_in: true, issue: true }
    }
}

/// An argument or environment string as C wants it; one with a NUL in it
/// becomes empty.
fn c_string(text: &str) -> CString {
    CString::new(text).unwrap_or_default()
}

/// serial-console(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    let command_line = std::fs::read_to_string("/kernel/cmdline").unwrap_or_default();
    let Some(format) = asked(&command_line) else {
        // Spark starts it at every boot; with no serial console asked for,
        // it has nothing to do, and stays stopped (restart = on-failure).
        eprintln!("serial-console: the kernel was not started with {FLAG}: no serial console");
        return Ok(0);
    };
    let format = format.unwrap_or_else(|why| {
        eprintln!("serial-console: {why}; the line runs at 115200n8");
        Format::DEFAULT
    });
    if !Path::new(LINE).exists() {
        eprintln!("serial-console: no {LINE}: this computer has no serial port COM1");
        return Ok(0);
    }
    let passwd = std::fs::read_to_string(accounts::PASSWD).unwrap_or_default();
    let root = elevate::root_account(accounts::users(&passwd).entries().cloned());
    let sign_in = !util::kernel_flag("console-sign-in=off");
    let issue = std::fs::read_to_string(ISSUE).unwrap_or_else(|_| DEFAULT_ISSUE.into());
    let env = environment(&root.shell, &root.home);
    let mut once = util::kernel_flag("install").then(|| Session::once(INSTALLER));
    match (&once, sign_in) {
        (Some(_), _) => eprintln!("serial-console: install: the installer's screens on {LINE}"),
        (None, false) => eprintln!("serial-console: console-sign-in=off: root's shell on {LINE}"),
        (None, true) => eprintln!("serial-console: the sign-in on {LINE}"),
    }
    let mut quick_exits = 0;
    loop {
        let session = match once.take() {
            Some(installer) => installer,
            None if sign_in => Session::sign_in(),
            None => Session::shell(&root.shell),
        };
        let started = Instant::now();
        match run(&session, &env, format, &issue) {
            Ok(status) if started.elapsed() < QUICK => {
                eprintln!("serial-console: {} ended at once (status {status:#x})", session.path);
            }
            Ok(_) => {}
            Err(err) => eprintln!("serial-console: cannot start {} on {LINE}: {err}", session.path),
        }
        quick_exits = if started.elapsed() < QUICK { quick_exits + 1 } else { 0 };
        if quick_exits >= QUICK_MAX {
            eprintln!(
                "serial-console: {QUICK_MAX} sessions in a row ended at once; starting the next \
                 in {} seconds",
                PAUSE.as_secs()
            );
            std::thread::sleep(PAUSE);
            quick_exits = 0;
        }
    }
}

/// The sessions' environment: serial-console's own, from Spark, with what
/// a login sets, for root, whose sessions they are until someone signs in.
fn environment(shell: &str, home: &str) -> Vec<CString> {
    let set =
        [("TERM", TERM), ("HOME", home), ("SHELL", shell), ("USER", "root"), ("LOGNAME", "root")];
    std::env::vars()
        .filter(|(name, _)| !set.iter().any(|(n, _)| n == name))
        .map(|(name, value)| format!("{name}={value}"))
        .chain(set.iter().map(|(name, value)| format!("{name}={value}")))
        .map(|entry| c_string(&entry))
        .collect()
}

/// Runs one session on the line and waits for it to end. Its wait status.
fn run(session: &Session, env: &[CString], format: Format, issue: &str) -> io::Result<i32> {
    let line: OwnedFd = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC)
        .open(LINE)?
        .into();
    set_line(line.as_raw_fd(), format)?;
    let (rows, cols) = ask_size(line.as_raw_fd()).unwrap_or(FALLBACK_SIZE);
    let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCSWINSZ reads the winsize, which lives for the call.
    unsafe { libc::ioctl(line.as_raw_fd(), libc::TIOCSWINSZ, &raw const size) };
    if session.issue {
        // The terminal turns each newline into a return and a newline.
        write_all(line.as_raw_fd(), expand_issue(issue).as_bytes());
    }
    let pid = start(line.as_raw_fd(), session, env)?;
    // The session's programs hold the line now; serial-console keeps none
    // of it, so that the line closes when they are done.
    drop(line);
    let mut status = 0;
    loop {
        // SAFETY: waitpid writes the status.
        let waited = unsafe { libc::waitpid(pid, &raw mut status, 0) };
        if waited == pid {
            return Ok(status);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Sets the line as a new terminal is, at `format`'s speed and character
/// format: what the last session changed, a program left in raw mode or
/// a stty(1), is undone.
fn set_line(fd: RawFd, format: Format) -> io::Result<()> {
    // SAFETY: all zeros is a valid termios: numbers and an array of them.
    let mut settings: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: tcgetattr writes the termios it is given.
    if unsafe { libc::tcgetattr(fd, &raw mut settings) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let settings = new_terminal(settings, format);
    // SAFETY: tcsetattr reads the termios, which lives for the call.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw const settings) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `settings` made a new terminal's, as Radix makes one and as agetty(8)
/// leaves a line, at `format`'s speed and character format. CLOCAL keeps
/// the line up with no modem signals, and no HUPCL leaves DTR raised for
/// the kernel's log (ADR 0318).
fn new_terminal(mut settings: libc::termios, format: Format) -> libc::termios {
    settings.c_iflag = libc::ICRNL | libc::IXON | libc::IUTF8;
    settings.c_oflag = libc::OPOST | libc::ONLCR;
    settings.c_cflag = format.speed | format.size | format.parity | libc::CREAD | libc::CLOCAL;
    settings.c_lflag = libc::ISIG
        | libc::ICANON
        | libc::ECHO
        | libc::ECHOE
        | libc::ECHOK
        | libc::ECHOCTL
        | libc::ECHOKE
        | libc::IEXTEN;
    settings.c_cc = [0; libc::NCCS];
    for (index, key) in [
        (libc::VINTR, 0x03),    // Ctrl+C
        (libc::VQUIT, 0x1c),    // Ctrl+backslash
        (libc::VERASE, 0x7f),   // Backspace, as terminals send it
        (libc::VKILL, 0x15),    // Ctrl+U
        (libc::VEOF, 0x04),     // Ctrl+D
        (libc::VMIN, 1),        // a read waits for one byte,
        (libc::VTIME, 0),       // however long it takes
        (libc::VSTART, 0x11),   // Ctrl+Q
        (libc::VSTOP, 0x13),    // Ctrl+S
        (libc::VSUSP, 0x1a),    // Ctrl+Z
        (libc::VREPRINT, 0x12), // Ctrl+R
        (libc::VWERASE, 0x17),  // Ctrl+W
        (libc::VLNEXT, 0x16),   // Ctrl+V
    ] {
        settings.c_cc[index] = key;
    }
    settings.c_ispeed = format.speed;
    settings.c_ospeed = format.speed;
    settings
}

/// Asks the terminal at the other end of the line its size, as xterm's
/// resize(1) does. None if nothing answers in time, as nothing does on a
/// line only a log reader is on.
fn ask_size(fd: RawFd) -> Option<(u16, u16)> {
    // SAFETY: all zeros is a valid termios.
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: tcgetattr writes the termios it is given.
    if unsafe { libc::tcgetattr(fd, &raw mut saved) } != 0 {
        return None;
    }
    // The answer is read as it comes, not echoed and not a line.
    let mut quiet = saved;
    quiet.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
    quiet.c_iflag &= !(libc::ICRNL | libc::IXON);
    // SAFETY: tcsetattr and tcflush act on our descriptor of the line,
    // with the termios living for the call.
    unsafe {
        libc::tcsetattr(fd, libc::TCSANOW, &raw const quiet);
        libc::tcflush(fd, libc::TCIFLUSH);
    }
    write_all(fd, ASK_SIZE);
    let answer = read_answer(fd, Instant::now() + ANSWER_WAIT);
    // SAFETY: as above. What else came, or an answer that came late, is
    // not typing.
    unsafe {
        libc::tcsetattr(fd, libc::TCSANOW, &raw const saved);
        libc::tcflush(fd, libc::TCIFLUSH);
    }
    size_in(&answer)
}

/// What the line received until a report's `R`, or until `deadline`.
fn read_answer(fd: RawFd, deadline: Instant) -> Vec<u8> {
    let mut answer = Vec::new();
    while !answer.contains(&b'R') && answer.len() < 64 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let ms = i32::try_from(left.as_millis()).unwrap_or(i32::MAX).max(1);
        let mut wanted = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: poll reads and writes the one pollfd it is given.
        let ready = unsafe { libc::poll(&raw mut wanted, 1, ms) };
        if ready < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if ready <= 0 {
            break;
        }
        let mut buffer = [0u8; 64];
        // SAFETY: read writes at most the buffer's length into it.
        let n = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        let Ok(n) = usize::try_from(n) else { break };
        if n == 0 {
            break;
        }
        answer.extend_from_slice(&buffer[..n]);
    }
    answer
}

/// The rows and columns in cursor report `answer`, `ESC [ rows ; cols R`,
/// if it holds one of a plausible size.
fn size_in(answer: &[u8]) -> Option<(u16, u16)> {
    let at = answer.windows(2).rposition(|pair| pair == b"\x1b[")?;
    let report = &answer[at + 2..];
    let end = report.iter().position(|&b| b == b'R')?;
    let (rows, cols) = std::str::from_utf8(&report[..end]).ok()?.split_once(';')?;
    let (rows, cols): (u16, u16) = (rows.parse().ok()?, cols.parse().ok()?);
    (rows >= SMALLEST.0 && cols >= SMALLEST.1).then_some((rows, cols))
}

/// Writes all of `bytes` to the line; what cannot be written is dropped,
/// as nothing more can be done with it.
fn write_all(fd: RawFd, bytes: &[u8]) {
    let mut at = 0;
    while at < bytes.len() {
        // SAFETY: writes from our buffer.
        let n = unsafe { libc::write(fd, bytes[at..].as_ptr().cast(), bytes.len() - at) };
        match usize::try_from(n) {
            Ok(n) if n > 0 => at += n,
            _ if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => {}
            _ => break,
        }
    }
}

/// Starts `session` on the line, `fd`, not waiting for it. Its process.
fn start(fd: RawFd, session: &Session, env: &[CString]) -> io::Result<libc::pid_t> {
    // Everything the child uses is made before the fork.
    let path = c_string(&session.path);
    let name = c_string(&session.name);
    let argv = [name.as_ptr(), std::ptr::null()];
    let mut envp: Vec<*const libc::c_char> = env.iter().map(|e| e.as_ptr()).collect();
    envp.push(std::ptr::null());
    // SAFETY: serial-console has one thread; the child only takes the line
    // and execs, with what was made before the fork.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        // SAFETY: async-signal-safe calls on the child's own copy of the
        // descriptor and of the strings.
        unsafe {
            if session.sign_in {
                // A session of its own, with the line as 0 to 2 but not
                // controlling it: the person's session takes it.
                libc::setsid();
                for target in 0..3 {
                    libc::dup2(fd, target);
                }
            } else {
                // A session of its own that the line controls, on 0 to 2.
                libc::login_tty(fd);
            }
            // The line's own descriptor closes on exec.
            libc::execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr());
            libc::_exit(127);
        }
    }
    Ok(pid)
}

/// /system/settings/issue's text, with agetty(8)'s escapes: `\l` the
/// line's name, `\n` the computer's name, `\s` the system's, `\r` its
/// release, `\v` its version, `\m` the machine, `\\` a backslash, as
/// consoled(8) shows it on the screen's consoles.
fn expand_issue(issue: &str) -> String {
    // SAFETY: all zeros is a valid utsname: arrays of bytes.
    let mut names: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: uname fills the structure it is given.
    let named = unsafe { libc::uname(&raw mut names) } == 0;
    let field = |field: &[libc::c_char]| -> String {
        if !named {
            return String::new();
        }
        let bytes: Vec<u8> = field.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let mut out = String::new();
    let mut chars = issue.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('l') => out.push_str(LINE_NAME),
            Some('n') => out.push_str(&field(&names.nodename)),
            Some('s') => out.push_str(&field(&names.sysname)),
            Some('r') => out.push_str(&field(&names.release)),
            Some('v') => out.push_str(&field(&names.version)),
            Some('m') => out.push_str(&field(&names.machine)),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_console_tty_s0_asks_for_the_console() {
        assert_eq!(asked("live install quiet"), None);
        assert_eq!(asked("console=tty0 quiet"), None);
        assert_eq!(asked("console=ttyS1,9600"), None);
        assert_eq!(asked("console=ttyS00"), None);
        // After `--` the words are Spark's.
        assert_eq!(asked("quiet -- console=ttyS0"), None);
        assert_eq!(asked("live console=ttyS0 install"), Some(Ok(Format::DEFAULT)));
        // The last counts.
        let nine = Format { speed: libc::B9600, ..Format::DEFAULT };
        assert_eq!(asked("console=ttyS0 console=ttyS0,9600"), Some(Ok(nine)));
        assert_eq!(asked("console=ttyS0,9600 console=tty0"), Some(Ok(nine)));
    }

    #[test]
    fn linux_console_options_give_the_format() {
        assert_eq!(format_of("115200n8"), Ok(Format::DEFAULT));
        assert_eq!(format_of("115200"), Ok(Format::DEFAULT));
        assert_eq!(format_of("115200n8r"), Ok(Format::DEFAULT));
        assert_eq!(
            format_of("9600e7"),
            Ok(Format { speed: libc::B9600, parity: libc::PARENB, size: libc::CS7 })
        );
        assert_eq!(
            format_of("57600o"),
            Ok(Format {
                speed: libc::B57600,
                parity: libc::PARENB | libc::PARODD,
                size: libc::CS8
            })
        );
        for bad in ["", "n8", "230400", "115200x8", "115200n9", "115200n8rr", "fast"] {
            assert!(format_of(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_new_terminal_is_made_whatever_the_last_session_left() {
        // SAFETY: all zeros is a valid termios.
        let mut left: libc::termios = unsafe { std::mem::zeroed() };
        left.c_lflag = libc::ECHO;
        left.c_cflag = libc::B9600 | libc::CS7 | libc::HUPCL | libc::CRTSCTS;
        left.c_cc[libc::VINTR] = b'x';
        let made = new_terminal(left, Format::DEFAULT);
        assert_eq!(made.c_cflag, libc::B115200 | libc::CS8 | libc::CREAD | libc::CLOCAL);
        assert_ne!(made.c_lflag & libc::ICANON, 0);
        assert_ne!(made.c_iflag & libc::ICRNL, 0);
        assert_ne!(made.c_oflag & libc::ONLCR, 0);
        assert_eq!(made.c_cc[libc::VINTR], 0x03);
        assert_eq!(made.c_cc[libc::VERASE], 0x7f);
        assert_eq!(made.c_cc[libc::VMIN], 1);
        assert_eq!(made.c_ospeed, libc::B115200);
    }

    #[test]
    fn the_size_is_read_from_the_cursor_report() {
        assert_eq!(size_in(b"\x1b[50;132R"), Some((50, 132)));
        // Typing before it is passed over.
        assert_eq!(size_in(b"ab\x1b[24;80R"), Some((24, 80)));
        assert_eq!(size_in(b"\x1b[1;1R"), None);
        assert_eq!(size_in(b"\x1b[24;80"), None);
        assert_eq!(size_in(b"\x1b[24R"), None);
        assert_eq!(size_in(b""), None);
    }

    #[test]
    fn the_issue_names_the_line() {
        assert_eq!(expand_issue("on \\l, \\\\ \\q"), "on ttyS0, \\ q");
        assert!(!expand_issue("\\s \\r").contains('\\'));
    }
}
