// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A virtual console: its terminal, and the session on it, a login shell
//! on a pseudo-terminal of the console's size, started again when it ends,
//! as getty(8) starts login(1) again on Linux.
//!
//! A sign-in screen shows the web console's address and fingerprint as
//! admind(8) has written them when it is drawn. admind writes them once it
//! runs, which is after consoled has drawn the first screens, and again
//! when its certificate changes. A screen nobody has typed on is then drawn
//! again, by hanging up its sign-in program, which the console starts again
//! at once; one a person has typed on is never replaced. The log says
//! "ttyN: drawing the sign-in screen again for the web console's address"
//! when it happens.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

use crate::term::Terminal;
use crate::welcome;

/// A session that ends sooner than this after it started ended at once.
const QUICK: Duration = Duration::from_secs(2);
/// After this many sessions in a row ended at once, the console waits
/// before it starts another.
const QUICK_MAX: u32 = 5;
/// How long it waits then.
const PAUSE: Duration = Duration::from_secs(10);

/// What the health check says of a failed start, shown under the issue
/// text until the next start (sysupdate(8), ADR 0501).
const FAILURE: &str = "/system/live/sysupdate/failure";

/// The notices kept for the next sign-in, one file each: a backup that
/// failed or is overdue, an app with known flaws (ADR 0504). Shown after
/// a failed start's.
const NOTICES: &str = "/system/live/sign-in";

/// What each console's session runs.
pub struct Shell {
    /// The program's path.
    pub path: String,
    /// Whether it is login(1), which signs people in: it gets the
    /// terminal but not as its controlling terminal, which the session
    /// it starts takes (ADR 0250). Otherwise it is a shell, root's, run
    /// as a login shell with the terminal its own.
    pub sign_in: bool,
    /// The environment it gets, as `NAME=value`.
    pub env: Vec<String>,
    /// The text shown before it starts: /system/settings/issue's.
    pub issue: String,
    /// The welcome banner, shown in the issue's place above the sign-in
    /// prompt when welcome.toml(5) says so (ADR 0261).
    pub welcome: Option<crate::welcome::Welcome>,
}

/// One virtual console.
pub struct Console {
    /// Its number, from 1: the N of ttyN and Alt+FN.
    pub number: usize,
    /// What it shows.
    pub term: Terminal,
    master: Option<OwnedFd>,
    /// The session's shell, while it runs.
    child: Option<libc::pid_t>,
    /// The master has hung up: every descriptor of its terminal is
    /// closed, though the shell has not been reaped yet.
    hung_up: bool,
    started: Instant,
    quick_exits: u32,
    /// When to start the next session, while it waits after quick exits.
    pub retry_at: Option<Instant>,
    /// The terminal's name, as `pts/N`, while it has one.
    terminal: Option<String>,
    /// A program run as root in the shell's place for the console's next
    /// session only, as the installer's screens on tty1 under the kernel
    /// flag `install` (dukovo-boot(7)). The shell follows when it ends.
    pub once: Option<String>,
    /// A program run as root in the shell's place for every session, as
    /// Rescue's dukovo-rescue(8) under the kernel flag `rescue=`, which
    /// starts again when it ends.
    pub always: Option<String>,
    /// The program the session runs, or ran last.
    program: String,
    /// What the screen shows of the web console, and whether it may be
    /// drawn again; `NoSignIn` while no session runs.
    sign_in_screen: SignInScreen,
}

/// What a console's screen shows of the web console, and whether anyone
/// has typed on it.
#[derive(Debug, PartialEq, Eq)]
enum SignInScreen {
    /// The session is not a sign-in: root's shell, or a program run once,
    /// which draws its own screen.
    NoSignIn,
    /// A sign-in screen nobody has typed on, drawn with these lines about
    /// the web console: none while admind had written none.
    Untouched { web_console: String },
    /// A sign-in screen a person has typed on, which is never replaced.
    Typed,
    /// A sign-in screen whose program has been hung up so that the screen
    /// is drawn again, and has not ended yet.
    Redrawing,
}

impl SignInScreen {
    /// Whether the screen may be drawn again for other lines about the web
    /// console: a sign-in screen nobody has typed on, with nobody
    /// `signed_in` on its terminal.
    fn may_redraw(&self, signed_in: bool) -> bool {
        !signed_in && matches!(self, SignInScreen::Untouched { .. })
    }

    /// Whether the screen was drawn with other lines about the web console
    /// than `current`. Only a screen nobody has typed on is compared.
    fn shows_other_than(&self, current: &str) -> bool {
        matches!(self, SignInScreen::Untouched { web_console } if web_console != current)
    }
}

/// How many sessions in a row have ended at once, when a session that
/// lived `lived` has ended after `count` such sessions. A session ended to
/// draw its screen again says nothing about the program, so it leaves the
/// count as it is.
fn quick_exits_after(count: u32, lived: Duration, redrawn: bool) -> u32 {
    if redrawn {
        count
    } else if lived < QUICK {
        count + 1
    } else {
        0
    }
}

/// An argument or environment string as C wants it; one with a NUL in it
/// becomes empty.
fn c_string(text: &str) -> CString {
    CString::new(text).unwrap_or_default()
}

impl Console {
    /// Console `number`, of `cols` by `rows` cells, with no session yet.
    pub fn new(number: usize, cols: usize, rows: usize) -> Console {
        Console {
            number,
            term: Terminal::new(cols, rows),
            master: None,
            child: None,
            hung_up: false,
            started: Instant::now(),
            quick_exits: 0,
            retry_at: None,
            terminal: None,
            once: None,
            always: None,
            program: String::new(),
            sign_in_screen: SignInScreen::NoSignIn,
        }
    }

    /// The program the session runs, or ran last.
    pub fn program(&self) -> &str {
        &self.program
    }

    /// The terminal's name, as /processes gives it: `pts/N`.
    pub fn terminal(&self) -> Option<&str> {
        self.terminal.as_deref()
    }

    /// Whether someone is signed in: a session has the terminal as its
    /// controlling terminal. Sign-in itself does not take it (ADR 0250).
    pub fn signed_in(&self) -> bool {
        // SAFETY: tcgetsid only reads the terminal's session.
        self.master_fd().is_some_and(|fd| unsafe { libc::tcgetsid(fd) } > 0)
    }

    /// Whether the sign-in screen may be drawn again for a change of the
    /// web console's lines: the session is a sign-in nobody has typed on,
    /// and nobody is signed in on it.
    pub fn may_redraw(&self) -> bool {
        self.sign_in_screen.may_redraw(self.signed_in())
    }

    /// When the sign-in screen was drawn, if it may be drawn again for a
    /// change of the web console's lines.
    pub fn redrawable_since(&self) -> Option<Instant> {
        self.may_redraw().then_some(self.started)
    }

    /// Whether the sign-in screen was drawn with other lines about the web
    /// console than `lines`.
    pub fn web_console_differs(&self, lines: &str) -> bool {
        self.sign_in_screen.shows_other_than(lines)
    }

    /// Whether a session runs or waits to run.
    pub fn has_session(&self) -> bool {
        self.child.is_some() || self.retry_at.is_some()
    }

    /// The master's descriptor, while it can be read.
    pub fn master_fd(&self) -> Option<RawFd> {
        self.master.as_ref().filter(|_| !self.hung_up).map(AsRawFd::as_raw_fd)
    }

    /// The session's process.
    pub fn child(&self) -> Option<libc::pid_t> {
        self.child
    }

    /// Starts a session: shows the issue text, then runs the shell on a
    /// new pseudo-terminal as its controlling terminal. The console's
    /// [`once`](Console::once) program, if it has one, runs instead, as
    /// root and with no issue text, since it draws the whole screen.
    pub fn start(&mut self, shell: &Shell) -> io::Result<()> {
        self.retry_at = None;
        self.term.reset();
        let once = self.once.take().or_else(|| self.always.clone());
        self.program = once.clone().unwrap_or_else(|| shell.path.clone());
        // login(1) signs people in; a program run once is root's, as the
        // shell is with no sign-in.
        let sign_in = shell.sign_in && once.is_none();
        let number = self.number;
        let (cols, rows) = (self.term.cols(), self.term.rows());
        // Read each time, so that a certificate admind has changed shows;
        // the banner leaves the rows it takes.
        let web_console =
            if sign_in { welcome::web_console(welcome::WEB_CONSOLE, cols) } else { String::new() };
        let banner_rows = rows.saturating_sub(web_console.lines().count());
        let issue = match &shell.welcome {
            // A program run once draws the whole screen; until it starts
            // to, no cursor blinks on the empty one.
            _ if once.is_some() => "\x1b[?25l".to_string(),
            Some(welcome) if sign_in => {
                welcome.render(cols, banner_rows, &|text: &str| expand_issue(text, number))
            }
            _ => expand_issue(&shell.issue, number),
        };
        self.term.feed(issue.replace('\n', "\r\n").as_bytes());
        self.term.feed(web_console.replace('\n', "\r\n").as_bytes());
        // A failed start's notice, until the next start (ADR 0501).
        if once.is_none()
            && let Ok(notice) = std::fs::read_to_string(FAILURE)
        {
            self.term.feed(format!("{}\n", notice.trim_end()).replace('\n', "\r\n").as_bytes());
        }
        if once.is_none() {
            for notice in notices() {
                self.term.feed(format!("{}\n", notice.trim_end()).replace('\n', "\r\n").as_bytes());
            }
        }
        let size = libc::winsize {
            ws_row: self.term.rows() as u16,
            ws_col: self.term.cols() as u16,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let (mut master, mut slave) = (-1, -1);
        let mut pts = [0 as libc::c_char; 64];
        // SAFETY: openpty writes the two descriptors and the terminal's
        // name, which is far shorter than the buffer.
        let opened = unsafe {
            libc::openpty(
                &raw mut master,
                &raw mut slave,
                pts.as_mut_ptr(),
                std::ptr::null(),
                &raw const size,
            )
        };
        if opened != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openpty made both descriptors, which are ours alone.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        set_flags(master.as_raw_fd());
        // A login shell's name starts with a dash (sh(1)); login's and a
        // program's run once do not.
        let name = self.program.rsplit('/').next().unwrap_or("sh");
        let login_shell = !sign_in && once.is_none();
        let argv0 = if login_shell { format!("-{name}") } else { name.to_string() };
        let argv_strings = [c_string(&self.program), c_string(&argv0)];
        let argv = [argv_strings[1].as_ptr(), std::ptr::null()];
        // XDG_VTNR names the console, as on Linux: nukesh's [folders]
        // resume keeps a place for each (ADR 0286).
        let vtnr = format!("XDG_VTNR={}", self.number);
        let env_strings: Vec<CString> =
            shell.env.iter().map(|e| c_string(e)).chain(std::iter::once(c_string(&vtnr))).collect();
        let mut envp: Vec<*const libc::c_char> = env_strings.iter().map(|e| e.as_ptr()).collect();
        envp.push(std::ptr::null());
        // SAFETY: consoled has one thread; the child only makes the
        // terminal its own and execs, with what was built before the fork.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }
        if pid == 0 {
            // SAFETY: async-signal-safe calls on the child's own copies of
            // the descriptors and the strings built before the fork.
            unsafe {
                libc::signal(libc::SIGCHLD, libc::SIG_DFL);
                libc::close(master.as_raw_fd());
                if sign_in {
                    // A session of its own, leaving consoled's, with the
                    // terminal as 0 to 2 but not controlling it.
                    libc::setsid();
                    for fd in 0..3 {
                        libc::dup2(slave.as_raw_fd(), fd);
                    }
                    if slave.as_raw_fd() > 2 {
                        libc::close(slave.as_raw_fd());
                    }
                } else {
                    libc::login_tty(slave.as_raw_fd());
                }
                libc::execve(argv_strings[0].as_ptr(), argv.as_ptr(), envp.as_ptr());
                libc::_exit(127);
            }
        }
        drop(slave);
        self.terminal = pts_name(&pts);
        self.master = Some(master);
        self.child = Some(pid);
        self.hung_up = false;
        self.started = Instant::now();
        self.sign_in_screen =
            if sign_in { SignInScreen::Untouched { web_console } } else { SignInScreen::NoSignIn };
        Ok(())
    }

    /// Takes in what the session wrote. Returns false once the terminal's
    /// other side has closed.
    pub fn read(&mut self) -> bool {
        let Some(fd) = self.master_fd() else { return false };
        let mut buffer = [0u8; 16384];
        // A burst of output is drawn once: up to four buffers a turn.
        for _ in 0..4 {
            // SAFETY: reads into our buffer.
            let n = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
            if n > 0 {
                self.term.feed(&buffer[..n as usize]);
                continue;
            }
            let error = io::Error::last_os_error();
            if n < 0 && error.kind() == io::ErrorKind::WouldBlock {
                break;
            }
            if n < 0 && error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            // EOF, or EIO: no descriptor of the terminal is open.
            self.hung_up = true;
            return false;
        }
        let replies = self.term.take_replies();
        if !replies.is_empty() {
            self.write(&replies);
        }
        true
    }

    /// Makes the console `cols` by `rows` cells, as a new text size leaves
    /// it (ADR 0325), and tells its program by the terminal's window size,
    /// which sends it SIGWINCH.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.term.resize(cols, rows);
        let Some(fd) = self.master_fd() else { return };
        let size = libc::winsize {
            ws_row: self.term.rows() as u16,
            ws_col: self.term.cols() as u16,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: TIOCSWINSZ reads the winsize, which lives through the call.
        if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &raw const size) } < 0 {
            let error = io::Error::last_os_error();
            eprintln!("consoled: tty{}: cannot set the window size: {error}", self.number);
        }
    }

    /// Sends the keys a person typed, `bytes`, to the session. A sign-in
    /// screen they are typed on is never drawn again for a change of the web
    /// console's lines: the person may be halfway through their name.
    pub fn type_keys(&mut self, bytes: &[u8]) {
        if matches!(self.sign_in_screen, SignInScreen::Untouched { .. }) {
            self.sign_in_screen = SignInScreen::Typed;
        }
        self.write(bytes);
    }

    /// Sends `bytes` to the session's input: the keys typed, or what the
    /// terminal answers to the program. What the terminal has no room for is
    /// dropped, as a program that does not read its input loses keys typed
    /// at it.
    fn write(&mut self, bytes: &[u8]) {
        let Some(fd) = self.master_fd() else { return };
        let mut at = 0;
        while at < bytes.len() {
            // SAFETY: writes from our buffer.
            let n = unsafe { libc::write(fd, bytes[at..].as_ptr().cast(), bytes.len() - at) };
            if n > 0 {
                at += n as usize;
            } else if n == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                break;
            }
        }
    }

    /// Ends the sign-in program with a hangup, as a modem's hang-up ends a
    /// login on its line, so that [`ended`](Console::ended) starts the
    /// sign-in again at once, with a new screen. A program that ignores the
    /// hangup keeps its screen and is not sent another, since it would
    /// ignore that too.
    pub fn redraw(&mut self) -> io::Result<()> {
        let Some(pid) = self.child else { return Ok(()) };
        // SAFETY: kill only sends the signal to the child consoled started.
        if unsafe { libc::kill(pid, libc::SIGHUP) } < 0 {
            return Err(io::Error::last_os_error());
        }
        self.sign_in_screen = SignInScreen::Redrawing;
        Ok(())
    }

    /// The session's shell ended with wait status `status`: the terminal
    /// goes (hanging up what still uses it), and a new session starts, at
    /// once unless sessions keep ending at once; one ended by
    /// [`redraw`](Console::redraw) is not one that ended at once. Returns a
    /// line for the log when it waits.
    pub fn ended(&mut self, status: i32, shell: &Shell) -> Option<String> {
        self.child = None;
        self.master = None;
        self.terminal = None;
        self.hung_up = false;
        let redrawn = self.sign_in_screen == SignInScreen::Redrawing;
        self.sign_in_screen = SignInScreen::NoSignIn;
        self.quick_exits = quick_exits_after(self.quick_exits, self.started.elapsed(), redrawn);
        if self.quick_exits >= QUICK_MAX {
            self.quick_exits = 0;
            self.retry_at = Some(Instant::now() + PAUSE);
            let message = format!(
                "{} ended at once {QUICK_MAX} times on tty{} (last status {status:#x}); \
                 starting it again in {} seconds",
                self.program,
                self.number,
                PAUSE.as_secs()
            );
            self.term.feed(format!("\r\nconsoled: {message}\r\n").as_bytes());
            return Some(message);
        }
        match self.start(shell) {
            Ok(()) => None,
            Err(err) => {
                self.retry_at = Some(Instant::now() + PAUSE);
                Some(format!("cannot start {} on tty{}: {err}", self.program, self.number))
            }
        }
    }
}

/// The `pts/N` in a terminal's path, as openpty wrote it.
/// The notices in [`NOTICES`], by their names.
fn notices() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(NOTICES) else { return Vec::new() };
    let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    paths.iter().filter_map(|path| std::fs::read_to_string(path).ok()).collect()
}

fn pts_name(name: &[libc::c_char]) -> Option<String> {
    let bytes: Vec<u8> = name.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    let path = String::from_utf8(bytes).ok()?;
    let at = path.rfind("pts/")?;
    Some(path[at..].to_string())
}

/// Makes the master non-blocking and closed on exec.
fn set_flags(fd: RawFd) {
    // SAFETY: fcntl on our own descriptor.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
}

/// /system/settings/issue's text for console `number`, with agetty(8)'s
/// escapes: `\l` the terminal's name, `\n` the host's name, `\s` the
/// system's, `\r` its release, `\v` its version, `\m` the machine, `\\` a
/// backslash.
pub fn expand_issue(issue: &str, number: usize) -> String {
    // SAFETY: uname fills the structure.
    let mut names: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
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
            Some('l') => out.push_str(&format!("tty{number}")),
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
    use std::os::unix::process::{CommandExt, ExitStatusExt};

    use super::*;

    #[test]
    fn terminals_are_named_as_processes_names_them() {
        let name = |text: &str| -> Vec<libc::c_char> {
            text.bytes().map(|b| b as libc::c_char).chain([0, 7]).collect()
        };
        assert_eq!(pts_name(&name("/devices/pts/3")).as_deref(), Some("pts/3"));
        assert_eq!(pts_name(&name("/dev/pts/12")).as_deref(), Some("pts/12"));
        assert_eq!(pts_name(&name("")), None);
    }

    #[test]
    fn expands_the_issue_escapes() {
        assert_eq!(expand_issue("on \\l, \\\\ \\q", 3), "on tty3, \\ q");
        assert!(!expand_issue("\\s", 1).contains('\\'));
    }

    const WEB_CONSOLE: &str = " Web console: https://duke-nas.local:9090\n";
    const OTHER_WEB_CONSOLE: &str = " Web console: https://other.local:9090\n";
    const NOBODY_SIGNED_IN: bool = false;
    const SOMEONE_SIGNED_IN: bool = true;

    fn untouched(web_console: &str) -> SignInScreen {
        SignInScreen::Untouched { web_console: web_console.to_string() }
    }

    #[test]
    fn a_sign_in_screen_nobody_typed_on_is_drawn_again_for_other_lines() {
        let screen = untouched(WEB_CONSOLE);
        assert!(screen.may_redraw(NOBODY_SIGNED_IN));
        // A new certificate, a screen drawn before admind wrote the file,
        // and a console that admind has stopped.
        assert!(screen.shows_other_than(OTHER_WEB_CONSOLE));
        assert!(untouched("").shows_other_than(WEB_CONSOLE));
        assert!(screen.shows_other_than(""));
    }

    #[test]
    fn a_screen_that_shows_the_lines_as_they_are_is_left_alone() {
        assert!(!untouched(WEB_CONSOLE).shows_other_than(WEB_CONSOLE));
        assert!(!untouched("").shows_other_than(""));
    }

    #[test]
    fn a_screen_a_person_typed_on_is_left_alone() {
        assert!(!SignInScreen::Typed.may_redraw(NOBODY_SIGNED_IN));
        assert!(!SignInScreen::Typed.shows_other_than(OTHER_WEB_CONSOLE));
    }

    #[test]
    fn a_screen_someone_is_signed_in_on_is_left_alone() {
        assert!(!untouched(WEB_CONSOLE).may_redraw(SOMEONE_SIGNED_IN));
    }

    #[test]
    fn a_session_that_is_not_a_sign_in_is_left_alone() {
        assert!(!SignInScreen::NoSignIn.may_redraw(NOBODY_SIGNED_IN));
        assert!(!SignInScreen::NoSignIn.shows_other_than(WEB_CONSOLE));
    }

    #[test]
    fn a_screen_that_is_being_drawn_again_is_not_hung_up_a_second_time() {
        assert!(!SignInScreen::Redrawing.may_redraw(NOBODY_SIGNED_IN));
        assert!(!SignInScreen::Redrawing.shows_other_than(OTHER_WEB_CONSOLE));
    }

    /// A pseudo-terminal's two ends: the console's master, and the slave, in
    /// raw mode so that what the console sends is readable at once.
    fn terminal() -> (OwnedFd, OwnedFd) {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: openpty writes the two descriptors, and is asked for
        // nothing else.
        let opened = unsafe {
            libc::openpty(
                &raw mut master,
                &raw mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(opened, 0);
        // SAFETY: openpty made both descriptors, which are ours alone.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        // SAFETY: tcgetattr fills the termios, which cfmakeraw and tcsetattr
        // then read, on a terminal of ours.
        unsafe {
            let mut modes: libc::termios = std::mem::zeroed();
            libc::tcgetattr(slave.as_raw_fd(), &raw mut modes);
            libc::cfmakeraw(&raw mut modes);
            libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &raw const modes);
        }
        set_flags(master.as_raw_fd());
        (master, slave)
    }

    /// Waits up to two seconds for `fd` to have something to read.
    fn wait_readable(fd: &OwnedFd) {
        let mut waiting = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: poll reads and writes the one pollfd.
        let ready = unsafe { libc::poll(&raw mut waiting, 1, 2000) };
        assert_eq!(ready, 1, "nothing came within two seconds");
    }

    #[test]
    fn typed_keys_end_the_screens_wait_but_terminal_replies_do_not() {
        let (master, slave) = terminal();
        let mut console = Console::new(1, 80, 25);
        console.sign_in_screen = untouched(WEB_CONSOLE);
        // The program asks the terminal whether it works, and the console
        // answers on the terminal's input, as it does to a program that
        // asks where its cursor is.
        let mut buffer = [0u8; 16];
        // SAFETY: writes and reads go through buffers that live through the
        // calls.
        unsafe {
            assert_eq!(libc::write(slave.as_raw_fd(), b"\x1b[5n".as_ptr().cast(), 4), 4);
        }
        console.master = Some(master);
        wait_readable(console.master.as_ref().unwrap());
        assert!(console.read());
        wait_readable(&slave);
        // SAFETY: as above.
        let got =
            unsafe { libc::read(slave.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        assert_eq!(&buffer[..got as usize], b"\x1b[0n");
        assert!(console.may_redraw(), "a reply is not typing");
        assert!(console.web_console_differs(OTHER_WEB_CONSOLE));
        console.type_keys(b"a");
        wait_readable(&slave);
        // SAFETY: as above.
        let got =
            unsafe { libc::read(slave.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        assert_eq!(&buffer[..got as usize], b"a");
        assert!(!console.may_redraw());
        assert!(!console.web_console_differs(OTHER_WEB_CONSOLE));
    }

    #[test]
    fn keys_typed_where_there_is_no_sign_in_change_nothing() {
        let mut shell = Console::new(2, 80, 25);
        shell.type_keys(b"ls\n");
        assert_eq!(shell.sign_in_screen, SignInScreen::NoSignIn);
    }

    #[test]
    fn the_sign_in_program_is_hung_up_and_its_screen_marked() {
        let mut command = std::process::Command::new("sleep");
        command.arg("30");
        // SAFETY: the closure runs between fork and exec and only sets a
        // signal's disposition, which touches no memory. The test may itself
        // run with hangups ignored, which the program would keep across
        // exec.
        unsafe {
            command.pre_exec(|| {
                libc::signal(libc::SIGHUP, libc::SIG_DFL);
                Ok(())
            });
        }
        let mut program = command.spawn().unwrap();
        let mut console = Console::new(1, 80, 25);
        console.sign_in_screen = untouched(WEB_CONSOLE);
        console.child = Some(libc::pid_t::try_from(program.id()).unwrap());
        console.redraw().unwrap();
        assert_eq!(console.sign_in_screen, SignInScreen::Redrawing);
        assert!(!console.may_redraw());
        assert_eq!(program.wait().unwrap().signal(), Some(libc::SIGHUP));
    }

    #[test]
    fn sessions_that_end_at_once_are_counted() {
        let short = QUICK / 2;
        assert_eq!(quick_exits_after(0, short, false), 1);
        assert_eq!(quick_exits_after(3, short, false), 4);
        assert_eq!(quick_exits_after(3, QUICK, false), 0);
        assert_eq!(quick_exits_after(3, QUICK * 5, false), 0);
    }

    #[test]
    fn a_session_ended_to_draw_its_screen_again_does_not_count() {
        let short = QUICK / 2;
        assert_eq!(quick_exits_after(0, short, true), 0);
        // Nor does it end a run of sessions that ended at once.
        assert_eq!(quick_exits_after(3, short, true), 3);
        assert_eq!(quick_exits_after(3, QUICK * 5, true), 3);
    }
}
