// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! consoled(8): Dukovo's virtual consoles (ADR 0100).
//!
//! The kernel draws its log on the screen until consoled takes the screen
//! (_/devices/fb0_, radix-framebuffer(2)). Then consoled draws six consoles on
//! it, one at a time, each a terminal for TERM=linux (`term`) with
//! login(1)'s sign-in on a pseudo-terminal (`session`, ADR 0250), or with
//! `--shell`, root's shell and no sign-in, and turns the keys read from
//! _/devices/kbd_ (radix-input(2)) into what the terminal sends (`keys`).
//! Alt+F1 to Alt+F6 show a console, starting its session the first time;
//! Shift+Page Up looks back; Scroll Lock holds the shown console's output.
//! elevate(8)'s password, in a pane on a console, is asked by consoled on
//! the bottom line, where no program draws or reads (`prompts`, ADR
//! 0251). Ctrl+Alt+Delete, which no program ever receives, brings that
//! prompt back cleared, or else consoled's own menu: sign in, on a console
//! where nobody is, restart or power off. The keys type in the layout
//! _/system/settings/keyboard_ names (keymap(1), ADR 0149), read again on
//! SIGHUP, which `service consoled reload` sends. With the kernel flag
//! `install`, tty1 runs the installer's screens before its shell
//! (dukovo-install(8)), in the largest font they fit, unless `console=ttyS0`
//! puts them on the serial port, where serial-console(8) runs them (ADR
//! 0318). Notifications from
//! notifyd(8) show on the bottom line (`notices`, ADR 0212). A braille
//! display reads the shown console, and types on it, through BRLTTY
//! (`braille`, ADR 0326). When
//! consoled ends, the screen is the kernel log's again. Spark starts it as
//! a service (spark(8)).

mod access;
mod braille;
mod keys;
mod notices;
mod prompts;
mod reader;
mod screen;
mod session;
mod sys;
mod term;
mod welcome;

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Instant;

use access::{Access, Size};
use braille::Braille;
use keymap::Keymap;
use keys::{Action, Keyboard, SlowKeys};
use notices::Notices;
use prompts::{Prompts, Typed};
use radix_abi::input::{Event, PRESSED, SET_LEDS};
use reader::Reader;
use screen::Screen;
use session::{Console, Shell};

const USAGE: &str = "usage: consoled [--consoles N] [--shell PATH]";

/// The consoles there are, at most: Alt+F1 to Alt+F12.
const CONSOLES_MAX: usize = 12;

/// Where the text shown before each session comes from.
const ISSUE: &str = "/system/settings/issue";

/// What is shown when /system/settings/issue is missing.
const DEFAULT_ISSUE: &str = "\\s \\r (\\l)\n\n";

/// What the Ctrl+Alt+Delete menu's restart runs.
const REBOOT: &str = "/system/commands/reboot";

/// What the Ctrl+Alt+Delete menu's power off runs.
const POWEROFF: &str = "/system/commands/poweroff";

/// What runs on each console: the sign-in.
const LOGIN: &str = "/system/commands/login";

/// What tty1 runs first under the kernel flag `install`.
const INSTALLER: &str = "/system/commands/dukovo-install";

/// Rescue's screens (dukovo-rescue(8)), on every console under the
/// kernel flag `rescue=` (ADR 0460).
const RESCUE: &str = "/system/commands/dukovo-rescue";

/// The columns and rows the installer's screens are drawn for: under the
/// kernel flag `install`, the font is the largest that gives this many
/// (ADR 0402), where the console's own wants 100 by 40.
const INSTALLER_GRID: (usize, usize) = (100, 30);

/// welcome.toml(5), when the banner shows on this boot. A file that is not
/// there is the defaults; its mistakes are logged, and leave their keys at
/// the defaults.
fn read_welcome() -> Option<welcome::Welcome> {
    let (welcome, problems) = match std::fs::read_to_string(welcome::SETTINGS) {
        Ok(text) => welcome::Welcome::parse(&text),
        Err(_) => (welcome::Welcome::default(), Vec::new()),
    };
    for problem in problems {
        eprintln!("consoled: {}: {problem}", welcome::SETTINGS);
    }
    welcome::shows_this_boot(&welcome, welcome::SHOWN).then_some(welcome)
}

/// The write end of the pipe the SIGCHLD handler wakes the loop with.
static WAKE: AtomicI32 = AtomicI32::new(-1);

/// Set by SIGHUP: read the keyboard layout's setting again.
static RELOAD: AtomicBool = AtomicBool::new(false);

extern "C" fn on_child(_: libc::c_int) {
    let fd = WAKE.load(Ordering::Relaxed);
    // SAFETY: write(2) is async-signal-safe; a full pipe already wakes.
    unsafe { libc::write(fd, [0u8].as_ptr().cast(), 1) };
}

extern "C" fn on_hangup(signal: libc::c_int) {
    RELOAD.store(true, Ordering::Relaxed);
    on_child(signal);
}

/// The layout the setting names, saying which; US when it cannot be had.
fn chosen_keymap() -> Keymap {
    let loaded = keymap::chosen().and_then(|name| Ok((keymap::load(&name)?, name)));
    match loaded {
        Ok((keymap, name)) => {
            eprintln!("consoled: the keys type in {} ({name})", keymap.name);
            keymap
        }
        Err(err) => {
            eprintln!("consoled: {err}; the keys type in English (US)");
            Keymap::us()
        }
    }
}

/// The options.
struct Options {
    consoles: usize,
    shell: Option<String>,
}

fn options() -> Result<Options, String> {
    let mut options = Options { consoles: 6, shell: None };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--consoles" => {
                let n = args.next().and_then(|n| n.parse().ok());
                options.consoles = n
                    .filter(|n| (1..=CONSOLES_MAX).contains(n))
                    .ok_or(format!("--consoles needs a number from 1 to {CONSOLES_MAX}"))?;
            }
            "--shell" => options.shell = Some(args.next().ok_or("--shell needs a path")?),
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--version" => {
                println!("consoled (Dukovo) {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown option `{other}`\n{USAGE}")),
        }
    }
    Ok(options)
}

/// Whether the kernel's command line has the flag `flag`
/// (dukovo-boot(7)), as `console-sign-in=off`, root's shell on each
/// console with no sign-in, or `install`, the installer's screens on tty1.
fn kernel_flag(flag: &str) -> bool {
    let line = std::fs::read_to_string("/kernel/cmdline").unwrap_or_default();
    has_flag(&line, flag)
}

/// Whether the kernel flags of command line `line` start Rescue:
/// `rescue=` and the root it rescues (dukovo-boot(7)).
fn rescue_flag(line: &str) -> bool {
    line.split_whitespace().take_while(|w| *w != "--").any(|w| w.starts_with("rescue="))
}

/// Whether the kernel flags of command line `line`, those before a lone
/// `--`, have `flag`.
fn has_flag(line: &str, flag: &str) -> bool {
    line.split_whitespace().take_while(|w| *w != "--").any(|w| w == flag)
}

/// Whether the kernel flags of command line `line` ask for a console on
/// the serial port, `console=ttyS0` with or without Linux's options after
/// a comma, which serial-console(8) keeps.
fn has_serial_console(line: &str) -> bool {
    line.split_whitespace().take_while(|w| *w != "--").any(|w| {
        w.strip_prefix("console=ttyS0").is_some_and(|r| r.is_empty() || r.starts_with(','))
    })
}

/// Whether the installer's screens run on the serial port rather than on
/// tty1: the kernel was started with `console=ttyS0`, as the install
/// stick's "Install Dukovo (serial console)" entry starts it, and the
/// computer has the port (ADR 0318).
fn installs_on_serial() -> bool {
    let line = std::fs::read_to_string("/kernel/cmdline").unwrap_or_default();
    has_serial_console(&line) && std::path::Path::new("/devices/ttyS0").exists()
}

/// Root's shell and home from the user database, as login(1) reads them.
fn root_account() -> (String, String) {
    // SAFETY: getpwuid returns a static entry or null.
    let entry = unsafe { libc::getpwuid(0) };
    let text = |p: *const libc::c_char| -> Option<String> {
        // SAFETY: the entry's strings are NUL-terminated while it lives.
        (!p.is_null())
            .then(|| unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned())
    };
    if entry.is_null() {
        return ("/system/commands/sh".into(), "/".into());
    }
    // SAFETY: not null, as checked.
    let entry = unsafe { &*entry };
    let shell =
        text(entry.pw_shell).filter(|s| !s.is_empty()).unwrap_or("/system/commands/sh".into());
    let home = text(entry.pw_dir).filter(|s| !s.is_empty()).unwrap_or("/".into());
    (shell, home)
}

/// The session's environment: consoled's own, with what a login sets.
fn session_env(shell: &str, home: &str) -> Vec<String> {
    let set = [
        ("TERM", "linux"),
        ("HOME", home),
        ("SHELL", shell),
        ("USER", "root"),
        ("LOGNAME", "root"),
    ];
    let mut env: Vec<String> = std::env::vars()
        .filter(|(name, _)| !set.iter().any(|(n, _)| n == name))
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    env.extend(set.iter().map(|(name, value)| format!("{name}={value}")));
    env
}

/// The SIGCHLD and SIGHUP handlers and the pipe they write to; returns the
/// pipe's read end.
fn watch_children() -> io::Result<File> {
    let mut fds = [0; 2];
    // SAFETY: pipe2 writes two descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    WAKE.store(fds[1], Ordering::Relaxed);
    // SAFETY: the handler only writes to the pipe, and the read end is
    // ours alone.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_child as *const () as usize;
        action.sa_flags = libc::SA_RESTART | libc::SA_NOCLDSTOP;
        libc::sigaction(libc::SIGCHLD, &raw const action, std::ptr::null_mut());
        action.sa_sigaction = on_hangup as *const () as usize;
        action.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGHUP, &raw const action, std::ptr::null_mut());
        Ok(File::from_raw_fd(fds[0]))
    }
}

/// Starts `path` with no arguments, not waiting for it.
fn run(path: &str) {
    match std::process::Command::new(path).spawn() {
        Ok(child) => eprintln!("consoled: started {path} as process {}", child.id()),
        Err(err) => eprintln!("consoled: cannot start {path}: {err}"),
    }
}

/// What consoled's own bottom line shows, over a notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bar {
    /// The Ctrl+Alt+Delete menu.
    Menu,
    /// A line that goes with the next key.
    Note(&'static str),
}

/// The note sticky keys turning on shows, and the reader says.
const STICKY_ON: &str =
    "Sticky keys on: Shift, Ctrl and Alt hold for the next key.    Shift five times: off";
/// The note sticky keys turning off shows.
const STICKY_OFF: &str = "Sticky keys off.    Shift five times: on";

struct Consoled {
    screen: Screen,
    consoles: Vec<Console>,
    active: usize,
    keyboard: Keyboard,
    kbd: Option<File>,
    shell: Shell,
    notices: Notices,
    /// Passwords asked for authd, while sign-in is on.
    prompts: Option<Prompts>,
    bar: Option<Bar>,
    /// What the screen reader says, while readerd runs (ADR 0324).
    reader: Reader,
    /// The console whose text is kept for the reader, while it listens.
    speaking: Option<usize>,
    /// The text size, sticky keys and slow keys (accessibility.toml(5),
    /// ADR 0325).
    access: Access,
    slow: SlowKeys,
    /// The braille display's connection, through BRLTTY (ADR 0326).
    braille: Braille,
}

impl Consoled {
    fn lights(&self) {
        if let Some(kbd) = &self.kbd {
            let _ =
                sys::control(kbd.as_raw_fd(), SET_LEDS, u64::from(self.keyboard.lights()), &mut []);
        }
    }

    /// Shows console `index`, starting its session if it has none.
    fn switch(&mut self, index: usize) {
        if index >= self.consoles.len() {
            return;
        }
        if !self.consoles[index].has_session() {
            self.start(index);
        }
        if index != self.active {
            self.active = index;
            self.screen.invalidate();
            let console = &mut self.consoles[index];
            self.reader.switched(console.number, &mut console.term);
        }
    }

    fn start(&mut self, index: usize) {
        let console = &mut self.consoles[index];
        match console.start(&self.shell) {
            Ok(()) => eprintln!(
                "consoled: started {} on tty{} as process {}",
                console.program(),
                console.number,
                console.child().unwrap_or(0)
            ),
            Err(err) => {
                eprintln!(
                    "consoled: cannot start {} on tty{}: {err}",
                    console.program(),
                    console.number
                );
                console.retry_at = Some(Instant::now() + std::time::Duration::from_secs(10));
            }
        }
    }

    /// A key event from the keyboard, as slow keys let it through.
    fn key_event(&mut self, event: &[u8], now: Instant) {
        let code = u16::from_le_bytes([event[Event::OFFSET_CODE], event[Event::OFFSET_CODE + 1]]);
        let value =
            u16::from_le_bytes([event[Event::OFFSET_VALUE], event[Event::OFFSET_VALUE + 1]]);
        for (code, value) in self.slow.event(code, value, now) {
            self.key(code, value);
        }
    }

    fn key(&mut self, code: u16, value: u16) {
        let modes = self.consoles[self.active].term.modes();
        // The screen reader's keys are its own, while it listens: the
        // reader key and those pressed with it.
        let console = &self.consoles[self.active];
        let value = match self.reader.key(code, value, console.number, &console.term) {
            reader::Key::Taken => return,
            reader::Key::Passed => value,
            reader::Key::Itself => PRESSED,
        };
        // A key takes a notification that waits for one off the line; the
        // key itself goes on as usual.
        if value == PRESSED {
            self.notices.key();
        }
        let action = self.keyboard.key(code, value, modes);
        self.act(action);
    }

    /// What a key does, from the keyboard or a braille display.
    fn act(&mut self, action: Action) {
        match action {
            Action::None => {}
            // consoled's own line takes the keys first, as it shows: the
            // shown console's password prompt, then the menu. What is
            // typed there is never said.
            Action::Send(bytes)
                if self
                    .prompts
                    .as_mut()
                    .is_some_and(|p| p.key(self.active, &bytes) == Typed::Taken) =>
            {
                self.reader.forget();
            }
            Action::Send(bytes) if self.bar.is_some() => {
                self.reader.forget();
                self.bar_key(&bytes);
            }
            Action::Send(bytes) => {
                let console = &mut self.consoles[self.active];
                console.term.reset_view();
                console.write(&bytes);
            }
            Action::Switch(index) => self.switch(index),
            Action::Page(direction) => {
                let half = (self.screen.rows() / 2).max(1) as isize;
                self.consoles[self.active].term.scroll_view(direction * half);
            }
            // What consoled shows next is said, not where the cursor was.
            Action::Attention => {
                self.reader.forget();
                self.attention();
            }
            Action::Lights => self.lights(),
            Action::TextSize(step) => self.text_size(step),
            Action::Sticky(on) => {
                eprintln!("consoled: sticky keys {}", if on { "on" } else { "off" });
                self.access.sticky = on;
                if let Err(err) = access::save("sticky-keys", if on { "true" } else { "false" }) {
                    eprintln!("consoled: {}: {err}", access::SETTINGS);
                }
                self.bar = Some(Bar::Note(if on { STICKY_ON } else { STICKY_OFF }));
            }
            Action::Latch(word) => self.reader.now(word),
        }
    }

    /// What a braille display's keys ask for (ADR 0326): a key, as if
    /// typed on the keyboard, or another console.
    fn braille_input(&mut self, input: braille::Input) {
        match input {
            braille::Input::Key { name, shift, ctrl, alt } => {
                let modes = self.consoles[self.active].term.modes();
                match keys::named(&name, (shift, ctrl, alt), modes) {
                    Some(action) => {
                        self.notices.key();
                        self.act(action);
                    }
                    None => {
                        eprintln!("consoled: the braille display sent {name}, which no key types");
                    }
                }
            }
            braille::Input::Switch(number) => self.switch(number - 1),
        }
    }

    /// Sends the braille display the shown console and the bottom line,
    /// `bottom`, if they changed.
    fn show_braille(&mut self, bottom: Option<&[term::Cell]>) {
        if !self.braille.connected() {
            return;
        }
        // The cursor is on consoled's own line while it asks for an
        // answer there: a password, or a choice from its menu.
        let asking = if self.prompts.as_ref().is_some_and(|p| p.shown(self.active).is_some()) {
            Some(braille::Asking::Prompt)
        } else if self.bar == Some(Bar::Menu) {
            Some(braille::Asking::Menu)
        } else {
            None
        };
        let console = &self.consoles[self.active];
        self.braille.show(braille::screen(console.number, &console.term, bottom, asking));
    }

    /// Ctrl+Alt+Plus (`step` 1) or Ctrl+Alt+Minus (-1): the text a step
    /// bigger or smaller, kept in accessibility.toml(5).
    fn text_size(&mut self, step: isize) {
        let steps = self.access.size.steps().saturating_add_signed(step).min(2);
        let size = Size::from_steps(steps);
        if size == self.access.size {
            self.reader.now(if step > 0 { "Text is at its largest" } else { "Text is normal" });
            return;
        }
        self.access.size = size;
        self.apply_size();
        if let Err(err) = access::save("text-size", &format!("\"{}\"", size.name())) {
            eprintln!("consoled: {}: {err}", access::SETTINGS);
        }
        self.reader.now(&format!("Text {}", size.name()));
    }

    /// Draws the text in the size the settings say, as far as the screen
    /// allows, each console as many cells as then fit.
    fn apply_size(&mut self) {
        match self.screen.set_size(self.access.size) {
            Ok(true) => {}
            Ok(false) => return,
            Err(err) => {
                eprintln!("consoled: cannot change the text size: {err}");
                return;
            }
        }
        eprintln!("consoled: text size {}", self.access.size.name());
        eprintln!("consoled: {}", self.screen.describe());
        let (cols, rows) = (self.screen.cols(), self.screen.rows());
        for console in &mut self.consoles {
            console.resize(cols, rows);
        }
    }

    /// accessibility.toml(5) again, on SIGHUP: its text size, sticky keys
    /// and slow keys from now on.
    fn reload_access(&mut self) {
        let access = Access::read();
        if access == self.access {
            return;
        }
        if access.sticky != self.access.sticky {
            self.keyboard.set_sticky(access.sticky);
            eprintln!("consoled: sticky keys {}", if access.sticky { "on" } else { "off" });
            self.reader.now(if access.sticky { "Sticky keys on" } else { "Sticky keys off" });
        }
        if access.slow != self.access.slow {
            self.slow.set(access.slow);
            match access.slow {
                Some(delay) => {
                    eprintln!(
                        "consoled: slow keys on: a key counts once held {} ms",
                        delay.as_millis()
                    );
                    self.reader.now("Slow keys on");
                }
                None => {
                    eprintln!("consoled: slow keys off");
                    self.reader.now("Slow keys off");
                }
            }
        }
        let resize = access.size != self.access.size;
        self.access = access;
        if resize {
            self.apply_size();
        }
    }

    /// Ctrl+Alt+Delete, which no program receives: the shown console's
    /// password prompt, cleared, if one waits there; consoled's menu
    /// otherwise.
    fn attention(&mut self) {
        let active = self.active;
        if self.prompts.as_mut().is_some_and(|p| p.clear(active)) {
            self.bar = None;
            return;
        }
        self.bar = Some(Bar::Menu);
    }

    /// A key's bytes while consoled's own line shows.
    fn bar_key(&mut self, bytes: &[u8]) {
        if self.bar != Some(Bar::Menu) {
            self.bar = None;
            return;
        }
        match bytes {
            b"\r" if self.shell.sign_in => {
                self.bar = None;
                self.sign_in();
            }
            b"r" | b"R" => {
                self.bar = None;
                eprintln!("consoled: Ctrl+Alt+Delete menu: restarting the machine");
                run(REBOOT);
            }
            b"p" | b"P" => {
                self.bar = None;
                eprintln!("consoled: Ctrl+Alt+Delete menu: powering off the machine");
                run(POWEROFF);
            }
            b"\x1b" | b"\x03" => self.bar = None,
            _ => {}
        }
    }

    /// Shows the sign-in: on the shown console if nobody is signed in
    /// there, or else on the first one where nobody is, where consoled
    /// alone runs on the terminal.
    fn sign_in(&mut self) {
        let free = |c: &Console| !c.signed_in();
        let index = if free(&self.consoles[self.active]) {
            Some(self.active)
        } else {
            self.consoles.iter().position(free)
        };
        match index {
            Some(index) => {
                eprintln!("consoled: Ctrl+Alt+Delete menu: sign-in on tty{}", index + 1);
                self.switch(index);
            }
            None => self.bar = Some(Bar::Note("Someone is signed in on every console.")),
        }
    }

    /// What consoled draws on the bottom line, if anything: the shown
    /// console's prompt, its menu or note, or a notification.
    fn bottom(&self) -> Option<Vec<term::Cell>> {
        let cols = self.screen.cols();
        if let Some(ask) = self.prompts.as_ref().and_then(|p| p.shown(self.active)) {
            return Some(ask.cells(cols));
        }
        match self.bar {
            Some(Bar::Menu) => return Some(prompts::menu_cells(cols, self.shell.sign_in)),
            Some(Bar::Note(text)) => return Some(prompts::note_cells(cols, text)),
            None => {}
        }
        let here = self.consoles[self.active].terminal();
        self.notices
            .shown
            .as_ref()
            .filter(|shown| shown.terminal.is_none() || shown.terminal.as_deref() != here)
            .map(|shown| shown.cells(cols))
    }

    /// Tells the screen reader what changed: readerd started or went, the
    /// shown console's text, and the bottom line.
    fn speak(&mut self, now: Instant) {
        let connected = self.reader.connect(now);
        // Only the shown console's text is kept, and only while readerd
        // listens.
        let speaking = self.reader.connected().then_some(self.active);
        if self.speaking != speaking {
            if let Some(old) = self.speaking {
                self.consoles[old].term.set_speaking(false);
            }
            if let Some(new) = speaking {
                self.consoles[new].term.set_speaking(true);
            }
            self.speaking = speaking;
        }
        if !self.reader.connected() {
            return;
        }
        let console = &mut self.consoles[self.active];
        if connected {
            self.reader.screen(console.number, &mut console.term);
        }
        self.reader.written(&mut console.term, now);
        // consoled's own line, as it shows: its prompt, menu or note,
        // after the trusted sound; then a notification, as any other text.
        let ask = self.prompts.as_ref().and_then(|p| p.shown(self.active));
        self.reader.prompt(ask.map(|ask| (ask.id(), ask.spoken())));
        let bar = match self.bar {
            Some(Bar::Menu) => Some(prompts::menu_text(self.shell.sign_in)),
            Some(Bar::Note(text)) => Some(text),
            None => None,
        };
        self.reader.bar(bar.filter(|_| ask.is_none()));
        let here = self.consoles[self.active].terminal();
        let notice = self
            .notices
            .shown
            .as_ref()
            .filter(|shown| shown.terminal.is_none() || shown.terminal.as_deref() != here)
            .map(|shown| shown.text.as_str());
        self.reader.notice(notice);
    }

    /// Reaps the ended children and starts new sessions for them.
    fn reap(&mut self) {
        loop {
            let mut status = 0;
            // SAFETY: waitpid fills `status`.
            let pid = unsafe { libc::waitpid(-1, &raw mut status, libc::WNOHANG) };
            if pid <= 0 {
                return;
            }
            let Some(console) = self.consoles.iter_mut().find(|c| c.child() == Some(pid)) else {
                continue;
            };
            if let Some(line) = console.ended(status, &self.shell) {
                eprintln!("consoled: {line}");
            }
        }
    }

    fn run(&mut self, wake: &mut File) -> io::Result<()> {
        let mut events = vec![0u8; Event::SIZE * 32];
        loop {
            // The wake pipe, the keyboard, notifyd, authd's prompts,
            // readerd, the braille display's socket and its connection,
            // then each console's master.
            let mut fds =
                vec![libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
            fds.push(libc::pollfd {
                fd: self.kbd.as_ref().map_or(-1, AsRawFd::as_raw_fd),
                events: libc::POLLIN,
                revents: 0,
            });
            fds.push(libc::pollfd { fd: self.notices.fd(), events: libc::POLLIN, revents: 0 });
            let prompts_fd = self.prompts.as_ref().map_or(-1, Prompts::fd);
            fds.push(libc::pollfd { fd: prompts_fd, events: libc::POLLIN, revents: 0 });
            fds.push(libc::pollfd { fd: self.reader.fd(), events: libc::POLLIN, revents: 0 });
            let listener = self.braille.listener_fd();
            fds.push(libc::pollfd { fd: listener, events: libc::POLLIN, revents: 0 });
            let (display, wanted) = self.braille.client_fd();
            fds.push(libc::pollfd { fd: display, events: wanted, revents: 0 });
            for (i, console) in self.consoles.iter().enumerate() {
                let held = i == self.active && self.keyboard.scroll;
                let fd = console.master_fd().filter(|_| !held).unwrap_or(-1);
                fds.push(libc::pollfd { fd, events: libc::POLLIN, revents: 0 });
            }
            let now = Instant::now();
            let next = self
                .consoles
                .iter()
                .filter_map(|c| c.retry_at)
                .chain(self.notices.deadline())
                .chain(self.prompts.as_ref().and_then(Prompts::deadline))
                .chain(self.reader.deadline())
                .chain(self.slow.deadline())
                .min();
            let timeout =
                next.map_or(-1, |at| at.saturating_duration_since(now).as_millis() as i32);
            // SAFETY: `fds` holds valid pollfds.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
            if fds[0].revents != 0 {
                let mut drain = [0u8; 64];
                while matches!(wake.read(&mut drain), Ok(n) if n > 0) {}
            }
            if RELOAD.swap(false, Ordering::Relaxed) {
                self.keyboard.set_keymap(chosen_keymap());
                self.reload_access();
            }
            self.reap();
            if fds[1].revents != 0
                && let Some(kbd) = self.kbd.as_mut()
            {
                match kbd.read(&mut events) {
                    Ok(n) => {
                        let batch = events[..n].to_vec();
                        let now = Instant::now();
                        for event in batch.as_chunks::<{ Event::SIZE }>().0 {
                            self.key_event(event, now);
                        }
                    }
                    // Fell behind: the next read goes on with what is kept.
                    Err(err) if err.raw_os_error() == Some(libc::EPIPE) => {}
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                    Err(err) => {
                        eprintln!("consoled: cannot read the keyboard: {err}");
                        self.kbd = None;
                    }
                }
            }
            let now = Instant::now();
            // Keys held long enough, with slow keys.
            for (code, value) in self.slow.tick(now) {
                self.key(code, value);
            }
            if fds[2].revents != 0 {
                self.notices.read(now);
            }
            if fds[3].revents != 0
                && let Some(prompts) = self.prompts.as_mut()
            {
                let consoles = &self.consoles;
                prompts.read(now, |terminal| {
                    consoles.iter().position(|c| c.terminal() == Some(terminal))
                });
            }
            if fds[4].revents != 0 {
                self.reader.read(now);
            }
            if fds[5].revents != 0 {
                self.braille.accept();
            }
            if fds[6].revents & libc::POLLOUT != 0 {
                self.braille.flush();
            }
            if fds[6].revents & !libc::POLLOUT != 0 {
                for input in self.braille.read() {
                    self.braille_input(input);
                }
            }
            for (i, console) in self.consoles.iter_mut().enumerate() {
                if fds[7 + i].revents != 0 {
                    console.read();
                }
            }
            for i in 0..self.consoles.len() {
                if self.consoles[i].retry_at.is_some_and(|at| at <= now) {
                    self.start(i);
                }
            }
            self.notices.tick(now);
            if let Some(prompts) = self.prompts.as_mut() {
                prompts.tick(now);
            }
            self.speak(now);
            // Only the cells that changed are drawn.
            let bottom = self.bottom();
            self.screen.draw(&self.consoles[self.active].term, bottom.as_deref())?;
            self.show_braille(bottom.as_deref());
        }
    }
}

fn main() -> ExitCode {
    let options = match options() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("consoled: {message}");
            return ExitCode::from(2);
        }
    };
    let install = kernel_flag("install") && !installs_on_serial();
    if kernel_flag("install") && !install {
        eprintln!("consoled: install: the installer's screens are on the serial port, ttyS0");
    }
    let grid = if install { INSTALLER_GRID } else { radix_fbcon::MIN_GRID };
    let mut screen = match Screen::open("/devices/fb0", grid) {
        Ok(screen) => screen,
        Err(err) => {
            // No screen to draw on, as with a serial console only: nothing
            // to do, and nothing to start again.
            eprintln!("consoled: no screen for consoles (/devices/fb0: {err})");
            return ExitCode::SUCCESS;
        }
    };
    eprintln!("consoled: {}", screen.describe());
    // The text size accessibility.toml(5) asks for, before the consoles
    // take their size from the screen.
    let access = Access::read();
    match screen.set_size(access.size) {
        Ok(true) => {
            eprintln!("consoled: text size {}", access.size.name());
            eprintln!("consoled: {}", screen.describe());
        }
        Ok(false) => {}
        Err(err) => eprintln!("consoled: cannot change the text size: {err}"),
    }
    let kbd = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC)
        .open("/devices/kbd")
        .map_err(|err| eprintln!("consoled: no keyboard (/devices/kbd: {err})"))
        .ok();
    // Sign-in on each console, unless a shell was asked for, which then
    // runs as root with no sign-in.
    let (sign_in, path, home) = match options.shell {
        Some(path) => (false, path, root_account().1),
        None if kernel_flag("console-sign-in=off") => {
            eprintln!("consoled: console-sign-in=off: root's shell on each console, no sign-in");
            let (shell, home) = root_account();
            (false, shell, home)
        }
        None => (true, LOGIN.to_string(), "/".to_string()),
    };
    let issue = std::fs::read_to_string(ISSUE).unwrap_or_else(|_| DEFAULT_ISSUE.into());
    let welcome = sign_in.then(read_welcome).flatten();
    let shell = Shell { env: session_env(&path, &home), path, sign_in, issue, welcome };
    let mut wake = match watch_children() {
        Ok(wake) => wake,
        Err(err) => {
            eprintln!("consoled: cannot watch for ended sessions: {err}");
            return ExitCode::FAILURE;
        }
    };
    let (cols, rows) = (screen.cols(), screen.rows());
    let mut consoles: Vec<Console> =
        (1..=options.consoles).map(|n| Console::new(n, cols, rows)).collect();
    // The install stick's Install Dukovo entry: the installer's screens
    // on tty1, then root's shell there (ADR 0310).
    if install {
        eprintln!("consoled: install: the installer's screens on tty1");
        consoles[0].once = Some(INSTALLER.to_string());
    }
    // Rescue (ADR 0460): its screens on every console, again whenever
    // they end, never a shell no one signed in to.
    if rescue_flag(&std::fs::read_to_string("/kernel/cmdline").unwrap_or_default()) {
        eprintln!("consoled: rescue: Rescue's screens on every console");
        for console in &mut consoles {
            console.always = Some(RESCUE.to_string());
        }
    }
    let mut keyboard = Keyboard::new(chosen_keymap());
    keyboard.set_sticky(access.sticky);
    let mut slow = SlowKeys::default();
    slow.set(access.slow);
    if access.sticky {
        eprintln!("consoled: sticky keys on");
    }
    if let Some(delay) = access.slow {
        eprintln!("consoled: slow keys on: a key counts once held {} ms", delay.as_millis());
    }
    let notices = Notices::new(Instant::now());
    // Passwords are asked on the consoles only where people sign in;
    // root's shells need none.
    let prompts = shell.sign_in.then(|| Prompts::new(Instant::now()));
    let mut consoled = Consoled {
        screen,
        consoles,
        active: 0,
        keyboard,
        kbd,
        shell,
        notices,
        prompts,
        bar: None,
        reader: Reader::new(Instant::now()),
        speaking: None,
        access,
        slow,
        braille: Braille::listen(braille::SOCKET),
    };
    consoled.lights();
    consoled.switch(0);
    eprintln!("consoled: {} consoles; tty1 is shown", options.consoles);
    match consoled.run(&mut wake) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("consoled: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{has_flag, has_serial_console, rescue_flag};

    #[test]
    fn sign_in_is_off_only_by_a_kernel_flag() {
        let off = "console-sign-in=off";
        assert!(has_flag("lockdep console-sign-in=off qemu-exit", off));
        assert!(!has_flag("lockdep qemu-exit", off));
        assert!(!has_flag("console-sign-in=on", off));
        // After `--` the words are Spark's.
        assert!(!has_flag("qemu-exit -- console-sign-in=off", off));
    }

    #[test]
    fn the_installer_starts_only_by_its_own_word() {
        assert!(has_flag("live install console-sign-in=off quiet", "install"));
        assert!(!has_flag("live installer quiet", "install"));
        assert!(!has_flag("live -- install", "install"));
        assert!(rescue_flag("live rescue=0a1b2c quiet"));
        assert!(!has_flag("live rescue=0a1b2c", "rescue"));
        assert!(!rescue_flag("live rescue quiet -- rescue=0a1b2c"));
    }

    #[test]
    fn the_serial_console_is_asked_for_with_or_without_options() {
        assert!(has_serial_console("live install console=ttyS0 console-sign-in=off"));
        assert!(has_serial_console("console=tty0 console=ttyS0,115200n8"));
        assert!(!has_serial_console("console=tty0 console=ttyS1"));
        assert!(!has_serial_console("console=ttyS00"));
        assert!(!has_serial_console("live -- console=ttyS0"));
    }
}
