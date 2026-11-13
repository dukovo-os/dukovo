// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! serial-console(8): consoles on serial ports, for a computer with no
//! screen or one managed from afar (Phase 14, ADRs 0318, 0480 and 0481), as
//! agetty(8) and systemd's serial-getty keep them on Linux.
//!
//! Spark starts it at every boot. It chooses the ports to keep a console
//! on (`choose`), in this order:
//!
//! - under the kernel flag `serial-console=any`, as the install stick's
//!   "Install Dukovo (serial console)" entry starts the live system, every
//!   serial port;
//! - each port a `console=ttyS<n>` flag names, as Linux keeps one on each
//!   of its consoles: COM2 for a BMC's Serial-over-LAN is `ttyS1`, Intel
//!   AMT's `ttyS4`. The last `console=` is the primary console, as on
//!   Linux;
//! - with no `console=` flag and no `spcr=off`, the port the firmware sends
//!   its own console to (ACPI's SPCR), at the firmware's speed, as Linux
//!   takes it on Arm servers; it is never the primary console, so it
//!   never runs the installer or Rescue, which stay on tty1.
//!
//! With none of these it ends at once and stays stopped. A console runs in
//! a process of its own, which serial-console starts again should it end.
//! On each port it runs, and again each time it ends:
//!
//! - under the kernel flag `install`, on the primary console, the
//!   installer's screens, once, as consoled(8) runs them on tty1, which
//!   then leaves them to the line;
//! - under `install` and `serial-console=any`, on every port, `Press Enter
//!   to install Dukovo on this port (ttyS1).`: the first port Enter is
//!   pressed on runs the installer's screens, once, and the others say
//!   where it runs and go on as below. The port's claim is the file
//!   /system/live/serial-install, which is in memory, so that a restart
//!   frees it;
//! - under `rescue=`, on the primary console, Rescue's screens, and
//!   nothing on the others (`serial-console=any` is passed over);
//! - under `console-sign-in=off`, root's shell, with no sign-in;
//! - otherwise the issue text, /system/settings/issue, and the sign-in,
//!   login(1).
//!
//! Each session finds the line as a new terminal is, at the speed and
//! character format its `console=ttyS1,57600n8` gives (115200n8 when it
//! gives none, the line GRUB and the kernel log run), whatever the last
//! session left; with TERM=vt220, as Linux's serial consoles have; and
//! with the size of the terminal at the other end, which is asked for as
//! xterm's resize(1) asks, or 80 by 24 when nothing answers.
//!
//! serial-console holds no descriptor of a line while a session runs, so
//! that once the session's programs are done with it, the kernel's log
//! goes out on it again, if `serial-log=` gives it the log, until the next
//! session starts.
//!
//! Where to look when a port has no console: the log lines it writes
//! (`log serial-console`) say which ports it chose and why, and `serial`
//! shows each port with what it does at this start. The boot tests
//! serial-firmware and install-serial cover it.

use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::args::Args;
use crate::util::{self, Fail};
use crate::{elevate, serial};

/// The kernel flag that asks for a console, with a port's name and
/// Linux's options after a comma: `console=ttyS1,57600n8`.
const FLAG: &str = "console=";

/// The kernel flag that gives every port a console, which offers the
/// installer under `install` (ADR 0481).
const ANY: &str = "serial-console=any";

/// The kernel flag that keeps the firmware's console port from being a
/// console when no `console=` is given.
const SPCR_OFF: &str = "spcr=off";

/// What runs once under the kernel flag `install`.
const INSTALLER: &str = "/system/commands/dukovo-install";

/// What the port the installer runs on is claimed with: a file holding
/// that port's name, made by the first port to claim it, in memory so that
/// a restart frees it.
const CLAIM: &str = "/system/live/serial-install";

/// Rescue's screens (dukovo-rescue(8)), under the kernel flag `rescue=`
/// (ADR 0460).
const RESCUE: &str = "/system/commands/dukovo-rescue";

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
pub(crate) struct Format {
    /// The speed, as its termios code: `B115200`, one of [`SPEEDS`].
    speed: libc::speed_t,
    /// The parity's control flags: none, `PARENB`, or `PARENB | PARODD`.
    parity: libc::tcflag_t,
    /// The character size's: `CS7` or `CS8`.
    size: libc::tcflag_t,
}

impl Format {
    /// 115200n8: what GRUB and the kernel's log run the line at.
    const DEFAULT: Format = Format { speed: libc::B115200, parity: 0, size: libc::CS8 };

    /// Linux's console options for the line, whole: `57600n8`.
    pub(crate) fn options(self) -> String {
        let baud = SPEEDS.iter().find(|(_, code)| *code == self.speed).map_or(0, |(baud, _)| *baud);
        let parity = match self.parity {
            0 => 'n',
            parity if parity & libc::PARODD != 0 => 'o',
            _ => 'e',
        };
        let bits = if self.size == libc::CS7 { '7' } else { '8' };
        format!("{baud}{parity}{bits}")
    }
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

/// A serial port a `console=` flag names, and how it asks the port to
/// run.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Asked {
    /// The port's name: `ttyS1`.
    pub(crate) name: String,
    /// Its format, or why its options are not one.
    pub(crate) format: Result<Format, String>,
}

impl Asked {
    /// The options the port runs at: its format's, or 115200n8 when its
    /// own were not readable.
    pub(crate) fn options(&self) -> String {
        self.format.as_ref().copied().unwrap_or(Format::DEFAULT).options()
    }
}

/// The serial ports the kernel flags of command line `line`, those before
/// a lone `--`, ask for consoles on: each `console=ttyS<n>`, with the
/// options of its last flag, as on Linux, in the order of their last
/// flags. The last is the primary console if it is a serial port's.
fn consoles(line: &str) -> Vec<Asked> {
    let mut asked: Vec<Asked> = Vec::new();
    let flags = line.split_whitespace().take_while(|word| *word != "--");
    for value in flags.filter_map(|word| word.strip_prefix(FLAG)) {
        let (name, options) = match value.split_once(',') {
            Some((name, options)) => (name, Some(options)),
            None => (value, None),
        };
        if !is_serial_port(name) {
            continue;
        }
        let format = options.map_or(Ok(Format::DEFAULT), |options| format_of(name, options));
        asked.retain(|earlier| earlier.name != name);
        asked.push(Asked { name: name.to_string(), format });
    }
    asked
}

/// Whether `name` is a serial port's, as the kernel names them: `ttyS`
/// and a number, with no leading zero.
fn is_serial_port(name: &str) -> bool {
    name.strip_prefix("ttyS").is_some_and(|digits| {
        !digits.is_empty()
            && digits.bytes().all(|byte| byte.is_ascii_digit())
            && (digits == "0" || !digits.starts_with('0'))
    })
}

/// Which serial ports serial-console keeps a console on, and why.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// `serial-console=any`: every port, none of them the primary console.
    Any(Vec<Asked>),
    /// The ports the `console=` flags name.
    Flags(Vec<Asked>),
    /// The port the firmware sends its console to, which is no primary
    /// console.
    Firmware(Asked),
    /// No port: the serial console has nothing to do.
    Nothing,
}

/// The ports to keep a console on, by the kernel flags of command line
/// `line`, among the serial `ports` the kernel lists, by number, each with
/// the options of its line if the firmware sends its console there.
///
/// A `console=` flag of any kind, a screen's too, and `spcr=off` keep the
/// firmware's port from being a console, as they do in the kernel
/// (`CommandLine::firmware_console`), so that the line the kernel runs the
/// port at and the one its console runs at agree.
pub(crate) fn choose(line: &str, ports: &[(String, Option<String>)]) -> Choice {
    let console_flag =
        line.split_whitespace().take_while(|word| *word != "--").any(|word| word.starts_with(FLAG));
    let firmware = !console_flag && !util::kernel_flag_in(line, SPCR_OFF);
    let at_firmware_line = |name: &str, options: &str| Asked {
        name: name.to_string(),
        format: format_of(name, options),
    };
    // Rescue runs on the primary console alone.
    if util::kernel_flag_in(line, ANY) && util::kernel_word_in(line, "rescue=").is_none() {
        let every = ports.iter().map(|(name, options)| match options {
            Some(options) if firmware => at_firmware_line(name, options),
            _ => Asked { name: name.clone(), format: Ok(Format::DEFAULT) },
        });
        return Choice::Any(every.collect());
    }
    let asked = consoles(line);
    if !asked.is_empty() {
        return Choice::Flags(asked);
    }
    if firmware
        && let Some((name, Some(options))) = ports.iter().find(|(_, options)| options.is_some())
    {
        return Choice::Firmware(at_firmware_line(name, options));
    }
    Choice::Nothing
}

/// Whether the kernel flags of command line `line` make serial port
/// `name` the primary console: the last `console=` is its.
pub(crate) fn is_primary(line: &str, name: &str) -> bool {
    let flags = line.split_whitespace().take_while(|word| *word != "--");
    flags
        .filter_map(|word| word.strip_prefix(FLAG))
        .last()
        .is_some_and(|value| value.split_once(',').map_or(value, |(port, _)| port) == name)
}

/// The format Linux's console options `options` for port `name` give:
/// the speed in baud, then optionally the parity, `n`, `o` or `e`, and
/// the data bits, `7` or `8`, as in `115200n8`. A final `r`, RTS/CTS flow
/// control, is passed over: the PC's serial ports do none (ADR 0318).
pub(crate) fn format_of(name: &str, options: &str) -> Result<Format, String> {
    let flag = format!("{FLAG}{name}");
    let flag = flag.as_str();
    let digits = options.find(|c: char| !c.is_ascii_digit()).unwrap_or(options.len());
    let (speed, rest) = options.split_at(digits);
    let speed = speed
        .parse::<u32>()
        .ok()
        .and_then(|baud| SPEEDS.iter().find(|(speed, _)| *speed == baud))
        .map(|(_, code)| *code)
        .ok_or_else(|| {
            format!("{flag},{options}: the speed must be one of 300, 600, 1200, ... 57600, 115200")
        })?;
    let mut rest = rest.chars();
    let parity = match rest.next() {
        None | Some('n') => 0,
        Some('o') => libc::PARENB | libc::PARODD,
        Some('e') => libc::PARENB,
        Some(other) => {
            return Err(format!("{flag},{options}: the parity must be n, o or e, not {other}"));
        }
    };
    let size = match rest.next() {
        None | Some('8') => libc::CS8,
        Some('7') => libc::CS7,
        Some(other) => {
            return Err(format!("{flag},{options}: the data bits must be 7 or 8, not {other}"));
        }
    };
    // Only flow control may follow, which is passed over.
    let flow: String = rest.collect();
    if !flow.is_empty() && flow != "r" {
        return Err(format!("{flag},{options}: more than a speed, a parity, data bits and r"));
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

/// A serial port serial-console keeps a console on.
#[derive(Debug)]
struct Port {
    /// Its name, `ttyS1`, as the issue text's `\l` and the sign-in
    /// records give it.
    name: String,
    /// Its terminal: `/devices/ttyS1`.
    line: String,
    format: Format,
    /// What its console runs before its ordinary sessions.
    first: First,
}

/// What a console runs first, once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum First {
    /// Nothing: the ordinary sessions at once.
    Nothing,
    /// The installer's screens: the primary console under `install`.
    Installer,
    /// The offer of the installer, which runs here if this port claims it
    /// first (ADR 0481): every port under `install` and
    /// `serial-console=any`.
    Offer,
}

/// What the sessions on every port share.
struct Shared {
    /// Root's shell, which runs under `console-sign-in=off`.
    shell: String,
    /// Whether people sign in, rather than get root's shell.
    sign_in: bool,
    /// Whether Rescue's screens run, under `rescue=`.
    rescue: bool,
    /// /system/settings/issue's text.
    issue: String,
    env: Vec<CString>,
}

/// serial-console(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    let command_line = std::fs::read_to_string("/kernel/cmdline").unwrap_or_default();
    let listed: Vec<(String, Option<String>)> = serial::ports(Path::new(serial::TTY_CLASS))
        .into_iter()
        .map(|port| (port.name, port.firmware))
        .collect();
    let (asked, any) = match choose(&command_line, &listed) {
        Choice::Any(asked) => (asked, true),
        Choice::Flags(asked) => (asked, false),
        Choice::Firmware(port) => {
            eprintln!(
                "serial-console: the firmware's console (ACPI SPCR) is {}, at {}",
                port.name,
                port.options()
            );
            (vec![port], false)
        }
        Choice::Nothing => {
            // Spark starts it at every boot; with no serial console asked
            // for, it has nothing to do, and stays stopped (restart =
            // on-failure).
            eprintln!(
                "serial-console: no {FLAG} flag and no firmware console port: no serial console"
            );
            return Ok(0);
        }
    };
    let rescue = util::kernel_word("rescue=").is_some();
    let install = util::kernel_flag("install");
    let mut ports = Vec::new();
    for Asked { name, format } in asked {
        let format = format.unwrap_or_else(|why| {
            eprintln!("serial-console: {why}; {name} runs at 115200n8");
            Format::DEFAULT
        });
        let line = format!("/devices/{name}");
        if !Path::new(&line).exists() {
            eprintln!("serial-console: no {line}: this computer has no serial port {name}");
            continue;
        }
        let primary = !any && is_primary(&command_line, &name);
        let first = match (install, any, primary) {
            (true, true, _) => First::Offer,
            (true, false, true) => First::Installer,
            _ => First::Nothing,
        };
        // Rescue runs on the primary console alone.
        if primary || !rescue {
            ports.push(Port { name, line, format, first });
        }
    }
    if ports.is_empty() {
        return Ok(0);
    }
    let passwd = std::fs::read_to_string(accounts::PASSWD).unwrap_or_default();
    let root = elevate::root_account(accounts::users(&passwd).entries().cloned());
    let shared = Shared {
        env: environment(&root.shell, &root.home),
        shell: root.shell,
        sign_in: !util::kernel_flag("console-sign-in=off"),
        rescue,
        issue: std::fs::read_to_string(ISSUE).unwrap_or_else(|_| DEFAULT_ISSUE.into()),
    };
    keep(&ports, &shared)
}

/// Keeps a console on each of `ports`, each in a process of its own, and
/// starts one again should it end, with its ordinary sessions: what a
/// port runs first it runs once.
fn keep(ports: &[Port], shared: &Shared) -> ! {
    let mut consoles: Vec<(libc::pid_t, Instant)> =
        ports.iter().map(|port| start_console(port, shared, port.first)).collect();
    loop {
        let mut status = 0;
        // SAFETY: waitpid writes the status.
        let pid = unsafe { libc::waitpid(-1, &raw mut status, 0) };
        if pid < 0 {
            // Interrupted; there is always a console to wait for.
            continue;
        }
        let Some(index) = consoles.iter().position(|&(console, _)| console == pid) else {
            continue;
        };
        let port = &ports[index];
        eprintln!(
            "serial-console: the console on {} ended (status {status:#x}); starting it again",
            port.line
        );
        if consoles[index].1.elapsed() < QUICK {
            std::thread::sleep(PAUSE);
        }
        consoles[index] = start_console(port, shared, First::Nothing);
    }
}

/// Starts the console on `port` in a process of its own, trying again
/// every 10 seconds while the system has no room for one.
fn start_console(port: &Port, shared: &Shared, first: First) -> (libc::pid_t, Instant) {
    loop {
        // SAFETY: serial-console has one thread, so the child may go on
        // running its code.
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            serve(port, shared, first);
        }
        if pid > 0 {
            return (pid, Instant::now());
        }
        let err = io::Error::last_os_error();
        eprintln!(
            "serial-console: cannot start the console on {}: {err}; trying again in {} seconds",
            port.line,
            PAUSE.as_secs()
        );
        std::thread::sleep(PAUSE);
    }
}

/// The console on `port`: runs its sessions, one after another, for good,
/// after what `first` says.
fn serve(port: &Port, shared: &Shared, first: First) -> ! {
    let line = &port.line;
    let mut once = match first {
        First::Nothing => None,
        First::Installer => Some(Session::once(INSTALLER)),
        First::Offer => offer_installer(port).then(|| Session::once(INSTALLER)),
    };
    match (&once, shared.sign_in) {
        _ if shared.rescue => eprintln!("serial-console: rescue: Rescue's screens on {line}"),
        (Some(_), _) if first == First::Offer => {
            eprintln!("serial-console: install: the installer runs on {}", port.name);
        }
        (Some(_), _) => eprintln!("serial-console: install: the installer's screens on {line}"),
        (None, false) => eprintln!("serial-console: console-sign-in=off: root's shell on {line}"),
        (None, true) => eprintln!("serial-console: the sign-in on {line}"),
    }
    let mut quick_exits = 0;
    loop {
        let session = match once.take() {
            Some(installer) => installer,
            None if shared.rescue => Session::once(RESCUE),
            None if shared.sign_in => Session::sign_in(),
            None => Session::shell(&shared.shell),
        };
        let started = Instant::now();
        match run(port, &session, shared) {
            Ok(status) if started.elapsed() < QUICK => {
                eprintln!("serial-console: {} ended at once (status {status:#x})", session.path);
            }
            Ok(_) => {}
            Err(err) => eprintln!("serial-console: cannot start {} on {line}: {err}", session.path),
        }
        quick_exits = if started.elapsed() < QUICK { quick_exits + 1 } else { 0 };
        if quick_exits >= QUICK_MAX {
            eprintln!(
                "serial-console: {QUICK_MAX} sessions in a row ended at once on {line}; starting \
                 the next in {} seconds",
                PAUSE.as_secs()
            );
            std::thread::sleep(PAUSE);
            quick_exits = 0;
        }
    }
}

/// Offers the installer on `port`, and whether `port` has it: asks for
/// Enter, then claims the installer, which only the first port to do so
/// gets. A port that did not get it says where the installer runs.
fn offer_installer(port: &Port) -> bool {
    eprintln!("serial-console: install: Enter on {} starts the installer there", port.name);
    while let Err(err) = wait_for_enter(port) {
        eprintln!(
            "serial-console: install: cannot read {}: {err}; trying again in {} seconds",
            port.line,
            PAUSE.as_secs()
        );
        std::thread::sleep(PAUSE);
    }
    let claimed = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(CLAIM)
        .and_then(|mut file| file.write_all(port.name.as_bytes()));
    match claimed {
        Ok(()) => true,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            // The claim is made before the name is written, so the name
            // may not be there yet.
            let holder = std::fs::read_to_string(CLAIM).unwrap_or_default();
            let holder = if holder.is_empty() { "another port" } else { holder.trim() };
            tell(port, &format!("The installer is running on {holder}.\n"));
            false
        }
        Err(err) => {
            eprintln!(
                "serial-console: install: cannot claim the installer for {}: {err}",
                port.name
            );
            false
        }
    }
}

/// Writes the offer on `port`'s line and returns once a line has been
/// typed there, letting the line go again.
fn wait_for_enter(port: &Port) -> io::Result<()> {
    let mut line = open_line(port)?;
    // SAFETY: tcflush acts on our descriptor of the line. What the line
    // took before the offer, such as the line feed after the carriage
    // return that chose the menu's entry, is no one pressing Enter here.
    unsafe { libc::tcflush(line.as_raw_fd(), libc::TCIFLUSH) };
    let offer = format!("\nPress Enter to install Dukovo on this port ({}).\n", port.name);
    write_all(line.as_raw_fd(), offer.as_bytes());
    let mut typed = [0u8; 64];
    loop {
        match line.read(&mut typed) {
            Ok(n) if typed[..n].contains(&b'\n') || typed[..n].contains(&b'\r') => return Ok(()),
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
}

/// Writes `text` on `port`'s line, if it can be opened, and lets it go.
fn tell(port: &Port, text: &str) {
    match open_line(port) {
        Ok(line) => write_all(line.as_raw_fd(), text.as_bytes()),
        Err(err) => eprintln!("serial-console: cannot tell {}: {err}", port.line),
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

/// Runs one session on `port`'s line and waits for it to end. Its wait
/// status.
fn run(port: &Port, session: &Session, shared: &Shared) -> io::Result<i32> {
    let line: OwnedFd = open_line(port)?.into();
    let (rows, cols) = ask_size(line.as_raw_fd()).unwrap_or(FALLBACK_SIZE);
    let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCSWINSZ reads the winsize, which lives for the call.
    unsafe { libc::ioctl(line.as_raw_fd(), libc::TIOCSWINSZ, &raw const size) };
    if session.issue {
        // The terminal turns each newline into a return and a newline.
        write_all(line.as_raw_fd(), expand_issue(&shared.issue, &port.name).as_bytes());
        // A failed start's notice, until the next start (ADR 0501).
        if let Ok(notice) = std::fs::read_to_string(crate::sysupdate::health::NOTICE) {
            write_all(line.as_raw_fd(), format!("{}\n", notice.trim_end()).as_bytes());
        }
        // What its writers kept for the next sign-in (ADR 0504).
        for notice in crate::sign_in_notices::all() {
            write_all(line.as_raw_fd(), format!("{}\n", notice.trim_end()).as_bytes());
        }
    }
    let pid = start(line.as_raw_fd(), session, &shared.env)?;
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

/// Opens `port`'s line, not as its process's controlling terminal, set as
/// a new terminal is.
fn open_line(port: &Port) -> io::Result<File> {
    let line = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC)
        .open(&port.line)?;
    set_line(line.as_raw_fd(), port.format)?;
    Ok(line)
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
/// line's name, `line`, `\n` the computer's name, `\s` the system's, `\r` its
/// release, `\v` its version, `\m` the machine, `\\` a backslash, as
/// consoled(8) shows it on the screen's consoles.
fn expand_issue(issue: &str, line: &str) -> String {
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
            Some('l') => out.push_str(line),
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

    fn asked(name: &str, format: Format) -> Asked {
        Asked { name: name.to_string(), format: Ok(format) }
    }

    #[test]
    fn every_console_port_is_found_once_primary_last() {
        assert!(consoles("live install quiet").is_empty());
        assert!(consoles("console=tty0 quiet").is_empty());
        assert!(consoles("console=ttyS00 console=ttyS console=ttyS1x").is_empty());
        // After `--` the words are Spark's.
        assert!(consoles("quiet -- console=ttyS0").is_empty());
        assert_eq!(consoles("live console=ttyS0 install"), [asked("ttyS0", Format::DEFAULT)]);
        // A port's last flag counts, and its place among the others is
        // that of its last.
        let nine = Format { speed: libc::B9600, ..Format::DEFAULT };
        assert_eq!(consoles("console=ttyS0 console=ttyS0,9600"), [asked("ttyS0", nine)]);
        let line = "console=ttyS4 console=ttyS1,9600 console=tty0 console=ttyS4,115200n8";
        assert_eq!(consoles(line), [asked("ttyS1", nine), asked("ttyS4", Format::DEFAULT)]);
        assert_eq!(consoles("console=ttyS12"), [asked("ttyS12", Format::DEFAULT)]);
        // Options no port runs are said, not passed over.
        assert!(consoles("console=ttyS1,fast")[0].format.is_err());
        // The primary console is the last `console=`, a serial port's or
        // not.
        assert!(is_primary(line, "ttyS4"));
        assert!(!is_primary(line, "ttyS1"));
        assert!(!is_primary("console=ttyS0 console=tty0", "ttyS0"));
        assert!(is_primary("console=ttyS1,57600 quiet -- console=ttyS0", "ttyS1"));
        assert!(!is_primary("quiet", "ttyS0"));
    }

    fn ports(listed: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
        listed
            .iter()
            .map(|(name, options)| (name.to_string(), options.map(str::to_string)))
            .collect()
    }

    fn named(choice: &Choice) -> Vec<(&str, String)> {
        let asked = match choice {
            Choice::Any(asked) | Choice::Flags(asked) => asked.iter().collect(),
            Choice::Firmware(asked) => vec![asked],
            Choice::Nothing => Vec::new(),
        };
        asked.into_iter().map(|asked| (asked.name.as_str(), asked.options())).collect()
    }

    #[test]
    fn serial_console_any_gives_every_port_a_console_by_number() {
        let listed = ports(&[("ttyS0", None), ("ttyS1", Some("57600n8")), ("ttyS4", None)]);
        let choice = choose("live install serial-console=any console-sign-in=off", &listed);
        assert!(matches!(choice, Choice::Any(_)));
        // The firmware's port runs at the firmware's line, the others at 115200n8.
        let expected = [("ttyS0", "115200n8"), ("ttyS1", "57600n8"), ("ttyS4", "115200n8")];
        assert_eq!(named(&choice), expected.map(|(port, options)| (port, options.to_string())));
        // A console= flag turns the firmware's line off, as the kernel does, and no
        // port is the primary console.
        let choice = choose("serial-console=any console=ttyS0,9600", &listed);
        assert!(matches!(choice, Choice::Any(_)));
        assert_eq!(named(&choice)[1], ("ttyS1", "115200n8".to_string()));
        let choice = choose("serial-console=any spcr=off", &listed);
        assert_eq!(named(&choice)[1], ("ttyS1", "115200n8".to_string()));
        // Rescue runs on the primary console alone: the word is passed over.
        assert_eq!(
            choose("rescue=0a1b serial-console=any", &listed),
            Choice::Firmware(Asked {
                name: "ttyS1".to_string(),
                format: Ok(Format { speed: libc::B57600, ..Format::DEFAULT }),
            })
        );
        assert_eq!(
            choose("rescue=0a1b serial-console=any console=ttyS0", &listed),
            choose("console=ttyS0", &listed)
        );
        // After `--` the words are Spark's.
        assert!(!matches!(choose("quiet -- serial-console=any", &listed), Choice::Any(_)));
        assert!(!matches!(choose("quiet serial-console=all", &listed), Choice::Any(_)));
    }

    #[test]
    fn console_flags_beat_the_firmwares_port() {
        let listed = ports(&[("ttyS0", None), ("ttyS1", Some("57600n8"))]);
        let choice = choose("console=ttyS0,9600", &listed);
        assert!(matches!(choice, Choice::Flags(_)));
        assert_eq!(named(&choice), [("ttyS0", "9600n8".to_string())]);
        assert_eq!(
            choose("console=ttyS0,9600", &listed),
            Choice::Flags(consoles("console=ttyS0,9600"))
        );
    }

    #[test]
    fn the_firmwares_port_is_a_console_when_no_console_flag_says_otherwise() {
        let listed = ports(&[("ttyS0", None), ("ttyS1", Some("57600n8"))]);
        let choice = choose("lockdep console-sign-in=off", &listed);
        assert!(matches!(choice, Choice::Firmware(_)));
        assert_eq!(named(&choice), [("ttyS1", "57600n8".to_string())]);
        // After `--` the words are Spark's.
        assert!(matches!(choose("quiet -- console=ttyS0", &listed), Choice::Firmware(_)));
    }

    #[test]
    fn nothing_is_chosen_without_a_flag_a_firmware_port_or_with_either_turned_off() {
        let none = ports(&[("ttyS0", None), ("ttyS1", None)]);
        let firmware = ports(&[("ttyS0", None), ("ttyS1", Some("57600n8"))]);
        assert_eq!(choose("quiet", &none), Choice::Nothing);
        assert_eq!(choose("quiet", &[]), Choice::Nothing);
        assert_eq!(choose("spcr=off", &firmware), Choice::Nothing);
        // A console= of any kind, a screen's too, turns the firmware's port off.
        assert_eq!(choose("console=tty0", &firmware), Choice::Nothing);
        assert_eq!(choose("console=ttyS00", &firmware), Choice::Nothing);
        assert_eq!(choose("serial-console=any", &[]), Choice::Any(Vec::new()));
    }

    #[test]
    fn the_options_of_a_format_are_linuxs_whole_options() {
        for options in ["115200n8", "57600n8", "9600e7", "9600o8", "300n7"] {
            assert_eq!(format_of("ttyS0", options).unwrap().options(), options);
        }
        assert_eq!(format_of("ttyS0", "57600").unwrap().options(), "57600n8");
        assert_eq!(format_of("ttyS0", "57600n8r").unwrap().options(), "57600n8");
        assert_eq!(Format::DEFAULT.options(), "115200n8");
    }

    #[test]
    fn linux_console_options_give_the_format() {
        let format_of = |options| format_of("ttyS1", options);
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
        // The flag in the message is the port's own.
        assert!(format_of("fast").unwrap_err().starts_with("console=ttyS1,fast: "));
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
        assert_eq!(expand_issue("on \\l, \\\\ \\q", "ttyS4"), "on ttyS4, \\ q");
        assert!(!expand_issue("\\s \\r", "ttyS0").contains('\\'));
    }
}
