// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the tools share: errors, sizes and times as people read them, the
//! terminal, user names, signals, and the C library's regular expressions
//! and patterns.

use std::ffi::OsStr;
use std::fmt;
use std::io::{self, IsTerminal};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub use sysapi::pattern::{Regex, fnmatch};

/// Why a tool stopped: its exit status and what to say, if anything.
#[derive(Debug)]
pub struct Fail {
    pub code: i32,
    pub message: String,
    /// Whether it was a mistake in the command line, whose status is the
    /// tool's own (`main`'s USAGE_TWO).
    pub usage: bool,
}

/// A failure, with exit status 1.
pub fn fail(message: String) -> Fail {
    Fail { code: 1, message, usage: false }
}

/// A failure with exit status `code`.
pub fn fail_with(code: i32, message: String) -> Fail {
    Fail { code, message, usage: false }
}

/// A mistake in the command line: exit status 1, as most Linux tools
/// give, or 2 for those that say so.
pub fn usage(message: String) -> Fail {
    Fail { code: 1, message, usage: true }
}

impl From<io::Error> for Fail {
    fn from(error: io::Error) -> Fail {
        fail(error.to_string())
    }
}

/// `what: error`, as a failure.
// It takes the error as `map_err` hands it over.
#[allow(clippy::needless_pass_by_value)]
pub fn failed(what: &dyn fmt::Display, error: io::Error) -> Fail {
    fail(format!("{what}: {}", strip_os_error(&error)))
}

/// The program at `path` with the arguments `argv`, its name first, to
/// run or to start in place of this one as execv(3) would: a path
/// without a slash is in the working directory, never looked for in
/// `PATH`. `Command` gives it the default action for SIGPIPE and an empty
/// signal mask.
pub fn program<'a>(path: &[u8], mut argv: impl Iterator<Item = &'a [u8]>) -> Command {
    let path = if path.contains(&b'/') { path.to_vec() } else { [b"./", path].concat() };
    let mut command = Command::new(OsStr::from_bytes(&path));
    if let Some(name) = argv.next() {
        command.arg0(OsStr::from_bytes(name));
    }
    command.args(argv.map(OsStr::from_bytes));
    command
}

/// An error's text without Rust's " (os error N)".
pub fn strip_os_error(error: &io::Error) -> String {
    let text = error.to_string();
    match text.find(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

/// Splits `host[:port]`, or `[v6]:port`, taking `default` for a port
/// left out.
pub fn split_port(text: &str, default: u16) -> Result<(&str, u16), String> {
    let number = |port: &str| port.parse::<u16>().map_err(|_| format!("'{port}' is not a port"));
    if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or_else(|| format!("'{text}' has no ']'"))?;
        return match after.strip_prefix(':') {
            Some(port) => Ok((host, number(port)?)),
            None if after.is_empty() => Ok((host, default)),
            None => Err(format!("'{text}' is not an address")),
        };
    }
    match text.rsplit_once(':') {
        // An IPv6 address without brackets has no port.
        Some((host, _)) if host.contains(':') => Ok((text, default)),
        Some((host, port)) => Ok((host, number(port)?)),
        None => Ok((text, default)),
    }
}

/// A size as util-linux's fstrim and journalctl read it: bytes, or with
/// a suffix of K, M, G, T, P (KiB...) or KB, MB, GB, TB, PB (powers of
/// 1000).
pub fn parse_size(text: &str) -> Option<u64> {
    let digits = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    let (number, suffix) = text.split_at(digits);
    let number: u64 = number.parse().ok()?;
    let suffix = suffix.to_ascii_uppercase();
    let power = |base: u64, n: u32| number.checked_mul(base.checked_pow(n)?);
    // Not a size if a letter of it is not ASCII: logd reads one from its
    // settings, and must not end over it.
    let (letter, rest) = suffix.split_at_checked(suffix.len().min(1))?;
    let n = match letter {
        "" => return Some(number),
        "K" => 1,
        "M" => 2,
        "G" => 3,
        "T" => 4,
        "P" => 5,
        _ => return None,
    };
    match rest {
        "" | "IB" => power(1024, n),
        "B" => power(1000, n),
        _ => None,
    }
}

/// `bytes` with a unit, in powers of 1024 (`si`: of 1000), as `ls -h`
/// and `df -h` print them: rounded up, with one decimal below 10 and
/// none above, and carried into the next unit when it rounds up to one
/// (1048575 bytes is 1.0M).
pub fn human(bytes: u64, si: bool) -> String {
    let base: u128 = if si { 1000 } else { 1024 };
    let units = ["", "K", "M", "G", "T", "P", "E"];
    let bytes = u128::from(bytes);
    let (mut unit, mut scale) = (0, 1);
    while bytes >= scale * base && unit + 1 < units.len() {
        scale *= base;
        unit += 1;
    }
    // Rounded up, as coreutils does, so that a size never reads less.
    let tenths = (bytes * 10).div_ceil(scale);
    let whole = bytes.div_ceil(scale);
    if unit == 0 {
        bytes.to_string()
    } else if tenths < 100 {
        format!("{}.{}{}", tenths / 10, tenths % 10, units[unit])
    } else if whole == base && unit + 1 < units.len() {
        // Rounded up to the next unit, it is one of those.
        format!("1.0{}", units[unit + 1])
    } else {
        format!("{whole}{}", units[unit])
    }
}

/// Writes `text` to `file` whole, with mode `mode`: to `file.partial`
/// beside it, which is then renamed over it, so that a reader finds the
/// old text or the new and never part of one.
///
/// # Errors
///
/// If the file beside it cannot be written or renamed.
pub fn replace_whole(file: &std::path::Path, text: &str, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut partial = file.as_os_str().to_owned();
    partial.push(".partial");
    let partial = std::path::PathBuf::from(partial);
    std::fs::write(&partial, text)?;
    std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&partial, file)
}

/// Nanoseconds since the epoch, now.
pub fn now_ns() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
}

/// Nanoseconds since boot, now, on the monotonic clock the kernel's
/// times are on.
pub fn monotonic_ns() -> u64 {
    sysapi::time::monotonic_ns()
}

/// Seconds since the epoch, formatted with strftime(3) in local time.
pub fn local_time(seconds: i64, format: &str) -> String {
    time_text(seconds, format, false)
}

/// Seconds since the epoch, formatted with strftime(3) in UTC, or in
/// local time.
/// A time whose year does not fit is its seconds, and a format with a NUL
/// byte writes nothing.
pub fn time_text(seconds: i64, format: &str, utc: bool) -> String {
    match sysapi::time::format(seconds, format, utc) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => String::new(),
        Err(_) => seconds.to_string(),
    }
}

/// A duration in seconds, as ps(1)'s TIME: `[DD-]HH:MM:SS`.
pub fn clock_time(seconds: u64) -> String {
    let (days, rest) = (seconds / 86400, seconds % 86400);
    let (h, m, s) = (rest / 3600, rest % 3600 / 60, rest % 60);
    if days > 0 { format!("{days}-{h:02}:{m:02}:{s:02}") } else { format!("{h:02}:{m:02}:{s:02}") }
}

/// A duration in seconds, as ps(1)'s ELAPSED: `[[DD-]HH:]MM:SS`.
pub fn elapsed(seconds: u64) -> String {
    let (days, rest) = (seconds / 86400, seconds % 86400);
    let (h, m, s) = (rest / 3600, rest % 3600 / 60, rest % 60);
    if days > 0 {
        format!("{days}-{h:02}:{m:02}:{s:02}")
    } else if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// How long the system has been up, as uptime(1) says it: `5 min`,
/// `2:03`, `3 days, 2:03`.
pub fn up_for(seconds: u64) -> String {
    let days = seconds / 86400;
    let hours = seconds % 86400 / 3600;
    let minutes = seconds % 3600 / 60;
    let mut text = String::new();
    if days > 0 {
        text += &format!("{days} day{}, ", if days == 1 { "" } else { "s" });
    }
    if hours > 0 {
        text += &format!("{hours:2}:{minutes:02}");
    } else {
        text += &format!("{minutes} min");
    }
    text
}

/// The terminal's size on `fd`, in columns and rows, if it is one;
/// otherwise COLUMNS and LINES.
pub fn terminal_size(fd: impl AsFd) -> Option<(usize, usize)> {
    if let Ok(size) = sysapi::terminal::window_size(fd)
        && size.cols > 0
    {
        return Some((usize::from(size.cols), usize::from(size.rows)));
    }
    let var = |name: &str| std::env::var(name).ok().and_then(|v| v.parse().ok());
    Some((var("COLUMNS")?, var("LINES").unwrap_or(24)))
}

/// How many columns `text` takes on a terminal: two for each wide East
/// Asian character and emoji, none for combining marks and control
/// characters, one for the rest.
pub fn width(text: &str) -> usize {
    text.chars()
        .map(|c| match c as u32 {
            0..=0x1f | 0x7f..=0x9f | 0x300..=0x36f | 0x200b..=0x200f | 0xfe00..=0xfe0f => 0,
            0x1100..=0x115f
            | 0x2e80..=0x303e
            | 0x3041..=0x33ff
            | 0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xa000..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe30..=0xfe4f
            | 0xff00..=0xff60
            | 0xffe0..=0xffe6
            | 0x1f300..=0x1f64f
            | 0x1f900..=0x1f9ff
            | 0x20000..=0x3fffd => 2,
            _ => 1,
        })
        .sum()
}

/// Whether `fd` is a terminal.
pub fn is_terminal(fd: impl AsFd) -> bool {
    fd.as_fd().is_terminal()
}

/// Where the devices are (ADR 0113): the terminals' names are beneath
/// it, as `/devices/pts/0`.
pub const DEVICES: &str = "/devices";

/// A terminal's path as the tools show it, beneath the devices: `pts/0`
/// for `/devices/pts/0`, or for `/dev/pts/0`, through the link that has
/// Linux's name.
pub fn terminal_name(path: &str) -> &str {
    path.strip_prefix("/devices/").or_else(|| path.strip_prefix("/dev/")).unwrap_or(path)
}

/// Whether the locale's characters are UTF-8, as the first of
/// `LC_ALL`, `LC_CTYPE` and `LANG` that is set says (`C.UTF-8`,
/// `en_US.utf8`); with none, the C locale is ASCII.
pub fn utf8_locale() -> bool {
    let set = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
    set.is_some_and(|name| {
        let name = name.to_ascii_lowercase();
        name.contains("utf-8") || name.contains("utf8")
    })
}

/// uname(2)'s system name, release and machine.
pub fn uname() -> (String, String, String) {
    match sysapi::system::uname() {
        Ok(names) => (names.system, names.release, names.machine),
        Err(_) => ("Dukovo".into(), "unknown".into(), "x86_64".into()),
    }
}

/// The machine's name.
pub fn hostname() -> String {
    sysapi::system::host_name().unwrap_or_else(|_| "localhost".into())
}

/// User `uid`'s entry in the user database, if it has one.
fn user(uid: u32) -> Option<sysapi::users::User> {
    sysapi::users::user_by_id(uid).ok().flatten()
}

/// Whether the kernel was started with `flag` (dukovo-boot(7)): one of
/// the words of `/kernel/cmdline` before a lone `--`, which are the
/// kernel's, the rest being Spark's.
pub fn kernel_flag(flag: &str) -> bool {
    let line = std::fs::read_to_string("/kernel/cmdline").unwrap_or_default();
    kernel_flag_in(&line, flag)
}

/// The value of the kernel flag that starts with `prefix`, such as
/// `rescue=`, the last if there are several: what follows the prefix.
pub fn kernel_word(prefix: &str) -> Option<String> {
    let line = std::fs::read_to_string("/kernel/cmdline").unwrap_or_default();
    kernel_word_in(&line, prefix).map(str::to_string)
}

/// The value of the kernel flag of command line `line` that starts with
/// `prefix`.
pub(crate) fn kernel_word_in<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.split_whitespace().take_while(|w| *w != "--").filter_map(|w| w.strip_prefix(prefix)).last()
}

/// Whether the kernel flags of command line `line` have `flag`.
pub(crate) fn kernel_flag_in(line: &str, flag: &str) -> bool {
    line.split_whitespace().take_while(|w| *w != "--").any(|w| w == flag)
}

/// The user this program runs as.
pub fn own_user() -> u32 {
    sysapi::process::euid()
}

/// The user who asked: the one `elevate` ran this for, when it is run as
/// root through it, else this process's own.
pub fn asker() -> u32 {
    let elevated = std::env::var("SUDO_UID").ok().and_then(|uid| uid.parse().ok());
    match own_user() {
        0 => elevated.unwrap_or(0),
        own => own,
    }
}

/// The name of user `uid`, or its number if it has none.
pub fn user_name(uid: u32) -> String {
    user(uid).map_or_else(|| uid.to_string(), |user| user.name)
}

/// User `uid`'s home folder, as the user database gives it.
pub fn user_home(uid: u32) -> Option<String> {
    user(uid).map(|user| user.home.to_string_lossy().into_owned())
}

/// User `uid`'s login group, as the user database gives it.
pub fn login_group(uid: u32) -> Option<u32> {
    user(uid).map(|user| user.gid)
}

/// The name of group `gid`, or its number if it has none.
pub fn group_name(gid: u32) -> String {
    sysapi::users::group_by_id(gid)
        .ok()
        .flatten()
        .map_or_else(|| gid.to_string(), |group| group.name)
}

/// The user `name` names, by name or number.
pub fn user_id(name: &str) -> Option<u32> {
    if let Ok(uid) = name.parse() {
        return Some(uid);
    }
    sysapi::users::user_by_name(name).ok().flatten().map(|user| user.uid)
}

/// The group `name` names, by name or number.
pub fn group_id(name: &str) -> Option<u32> {
    if let Ok(gid) = name.parse() {
        return Some(gid);
    }
    sysapi::users::group_by_name(name).ok().flatten().map(|group| group.gid)
}

/// The signals, by number, as kill(1) names them.
pub const SIGNALS: &[(i32, &str)] = &[
    (libc::SIGHUP, "HUP"),
    (libc::SIGINT, "INT"),
    (libc::SIGQUIT, "QUIT"),
    (libc::SIGILL, "ILL"),
    (libc::SIGTRAP, "TRAP"),
    (libc::SIGABRT, "ABRT"),
    (libc::SIGBUS, "BUS"),
    (libc::SIGFPE, "FPE"),
    (libc::SIGKILL, "KILL"),
    (libc::SIGUSR1, "USR1"),
    (libc::SIGSEGV, "SEGV"),
    (libc::SIGUSR2, "USR2"),
    (libc::SIGPIPE, "PIPE"),
    (libc::SIGALRM, "ALRM"),
    (libc::SIGTERM, "TERM"),
    (libc::SIGCHLD, "CHLD"),
    (libc::SIGCONT, "CONT"),
    (libc::SIGSTOP, "STOP"),
    (libc::SIGTSTP, "TSTP"),
    (libc::SIGTTIN, "TTIN"),
    (libc::SIGTTOU, "TTOU"),
    (libc::SIGURG, "URG"),
    (libc::SIGXCPU, "XCPU"),
    (libc::SIGXFSZ, "XFSZ"),
    (libc::SIGVTALRM, "VTALRM"),
    (libc::SIGPROF, "PROF"),
    (libc::SIGWINCH, "WINCH"),
    (libc::SIGIO, "IO"),
    (libc::SIGSYS, "SYS"),
];

/// The signal `name` names: a number, or a name with or without `SIG`,
/// in any case (`IOT`, `POLL` and `CLD` are other names of three).
pub fn signal_number(name: &str) -> Option<i32> {
    if let Ok(number) = name.parse::<i32>() {
        return (0..65).contains(&number).then_some(number);
    }
    let upper = name.to_ascii_uppercase();
    let bare = upper.strip_prefix("SIG").unwrap_or(&upper);
    let bare = match bare {
        "IOT" => "ABRT",
        "POLL" => "IO",
        "CLD" => "CHLD",
        other => other,
    };
    SIGNALS.iter().find(|(_, n)| *n == bare).map(|(number, _)| *number)
}

/// Signal `number`'s name, without `SIG`.
pub fn signal_name(number: i32) -> Option<&'static str> {
    SIGNALS.iter().find(|(n, _)| *n == number).map(|(_, name)| *name)
}

/// Splits a list of the form ps(1) and pgrep(1) take: items separated by
/// commas or blanks.
pub fn list(text: &str) -> impl Iterator<Item = &str> {
    text.split([',', ' ', '\t']).filter(|item| !item.is_empty())
}

/// `bytes` as C would write a string: printable ASCII as it is, the
/// usual escapes, and `\ooo` or `\xhh` for the rest.
pub fn escape(bytes: &[u8], hex: bool) -> String {
    let mut text = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\n' => text.push_str("\\n"),
            b'\t' => text.push_str("\\t"),
            b'\r' => text.push_str("\\r"),
            b'"' => text.push_str("\\\""),
            b'\\' => text.push_str("\\\\"),
            0x20..=0x7e => text.push(b as char),
            _ if hex => text.push_str(&format!("\\x{b:02x}")),
            _ => text.push_str(&format!("\\{b:o}")),
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_flags() {
        assert!(kernel_flag_in("lockdep startup-trouble -- --services", "startup-trouble"));
        assert!(kernel_flag_in("startup-trouble", "startup-trouble"));
        assert!(!kernel_flag_in("lockdep -- startup-trouble", "startup-trouble"));
        assert!(!kernel_flag_in("startup-trouble=no", "startup-trouble"));
        assert!(!kernel_flag_in("", "startup-trouble"));
        let line = "live rescue=0a1b quiet -- rescue=spark";
        assert_eq!(kernel_word_in(line, "rescue="), Some("0a1b"));
        assert_eq!(kernel_word_in("live -- rescue=0a1b", "rescue="), None);
    }

    #[test]
    fn sizes_read_as_coreutils_prints_them() {
        assert_eq!(human(0, false), "0");
        assert_eq!(human(1023, false), "1023");
        assert_eq!(human(1024, false), "1.0K");
        assert_eq!(human(1536, false), "1.5K");
        assert_eq!(human(10 * 1024 * 1024, false), "10M");
        assert_eq!(human(1_000_000, true), "1.0M");
        // As the host's ls -lh and ls -l --si (coreutils 9.4) print them.
        for (bytes, binary, si) in [
            (1025, "1.1K", "1.1K"),
            (1100, "1.1K", "1.1K"),
            (10240, "10K", "11K"),
            (10241, "11K", "11K"),
            (999_999, "977K", "1.0M"),
            (1_047_552, "1023K", "1.1M"),
            (1_047_553, "1.0M", "1.1M"),
            (1_048_575, "1.0M", "1.1M"),
            (1_153_434, "1.2M", "1.2M"),
            (1_073_741_823, "1.0G", "1.1G"),
        ] {
            assert_eq!(human(bytes, false), binary, "{bytes}");
            assert_eq!(human(bytes, true), si, "{bytes}");
        }
        assert_eq!(human(u64::MAX, false), "16E");
        assert_eq!(human(u64::MAX, true), "19E");
    }

    #[test]
    fn durations() {
        assert_eq!(clock_time(3725), "01:02:05");
        assert_eq!(clock_time(90000), "1-01:00:00");
        assert_eq!(elapsed(65), "01:05");
        assert_eq!(elapsed(3725), "01:02:05");
        assert_eq!(up_for(300), "5 min");
        assert_eq!(up_for(7380), " 2:03");
        assert_eq!(up_for(86400 + 60), "1 day, 1 min");
    }

    #[test]
    fn signals_by_name_and_number() {
        assert_eq!(signal_number("TERM"), Some(libc::SIGTERM));
        assert_eq!(signal_number("sigkill"), Some(libc::SIGKILL));
        assert_eq!(signal_number("9"), Some(9));
        assert_eq!(signal_number("CLD"), Some(libc::SIGCHLD));
        assert_eq!(signal_number("NOPE"), None);
        assert_eq!(signal_name(libc::SIGHUP), Some("HUP"));
    }

    #[test]
    fn regular_expressions_and_patterns() {
        let regex = Regex::new("^ss?h(d|-agent)$", false).unwrap();
        assert!(regex.is_match("sshd"));
        assert!(regex.is_match("sh-agent"));
        assert!(!regex.is_match("bash"));
        assert!(Regex::new("(", false).is_err());
        assert!(Regex::new("SSHD", true).unwrap().is_match("sshd"));
        assert!(fnmatch("*.rs", OsStr::new("main.rs")));
        assert!(!fnmatch("*.rs", OsStr::new("main.c")));
    }

    #[test]
    fn escapes() {
        assert_eq!(escape(b"a\n\"\x01", false), "a\\n\\\"\\1");
        assert_eq!(escape(b"\xff", true), "\\xff");
    }
}
