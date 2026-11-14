// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The welcome banner above the sign-in prompt (ADR 0261): Duke and the
//! name drawn in plain ASCII, so that any console font has every
//! character, then the tagline and the system's name and version.
//! welcome.toml(5), `/system/settings/console/welcome.toml`, says when it
//! shows and what is in it. Where it shows, it takes the place of
//! /system/settings/issue's text; on a screen too small for Duke only the
//! two lines of words are shown. Under it, once admind(8) has written them,
//! come the web console's address and its certificate's fingerprint, so
//! that a person at the machine knows where to open the console and can
//! compare the fingerprint with the browser's warning. admind writes them
//! once it runs, after the first screens are drawn, and again when the
//! certificate changes, so consoled looks at their file (`WebConsoleWatch`)
//! while a sign-in screen nobody has typed on could use a change, and
//! `session` draws that screen again.

use std::fmt::Write as _;
use std::io::Read as _;
use std::time::{Duration, Instant, SystemTime};

/// The banner's settings.
pub const SETTINGS: &str = "/system/settings/console/welcome.toml";

/// Made on the first boot with `show = "first-boot"`; the banner is not
/// shown again once it is there.
pub const SHOWN: &str = "/system/state/consoled/welcome-shown";

/// Where admind writes the web console's address and the certificate's
/// fingerprint: `https://duke-nas.local:9090`, then `SHA-256 fingerprint
/// AB:CD:...`.
pub const WEB_CONSOLE: &str = "/system/live/admind/issue";

/// The most of that file consoled reads. It holds two short lines, and
/// admind, which parses what anyone sends it, writes it: consoled runs as
/// root and does not read without a bound what a service put at that path.
const WEB_CONSOLE_MAX: u64 = 1024;

/// How often consoled looks at that file while a sign-in screen could be
/// drawn again for a change in it: every 2 seconds in the 5 minutes after
/// the newest such screen was drawn, as admind writes the file soon after
/// boot, and then once a minute, for a certificate that is renewed, so
/// that a machine nobody signs in to is not woken often for nothing.
const LOOK_OFTEN: Duration = Duration::from_secs(2);
const LOOK_OFTEN_FOR: Duration = Duration::from_mins(5);
const LOOK_SELDOM: Duration = Duration::from_secs(60);

/// The bytes of a SHA-256 fingerprint, each shown as two hex digits.
const FINGERPRINT_BYTES: usize = 32;

/// Duke, the mark, as many columns wide as `DUKE_WIDTH`.
const DUKE: [&str; 12] = [
    r" |\            /|",
    r" | \   /\ /\  / |",
    r" |  \_/  V  \/  |__",
    r" |                 `-.",
    r" |  ____________      \",
    r" | |            `.     \",
    r" | |   `--.__      \     |",
    r" | |     (__)    __/     |",
    r" | |           (__      |",
    r" | |     `-.___.-'     /",
    r" | |______________,-' /",
    r" |__________________,-'",
];

/// The name, in letters four lines high.
const NAME: [&str; 4] = [
    r"    _          _",
    r" __| |  _  _  | |__  ___  __ __  ___",
    r"/ _` | | || | | / / / _ \ \ V / / _ \",
    r"\__,_|  \_,_| |_\_\ \___/  \_/  \___/",
];

/// Where the words beside Duke start, and the widest the banner is.
const DUKE_WIDTH: usize = 29;
const WIDTH: usize = DUKE_WIDTH + 38;

/// The tagline shown unless welcome.toml says another.
pub const TAGLINE: &str = "A bright future. A human future.";

/// The system's name and version, with issue(5)'s escapes.
pub const VERSION: &str = "\\s Server \\r";

/// When the banner shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    /// Above the sign-in prompt, every time it is shown.
    EveryBoot,
    /// Only during the system's first boot.
    FirstBoot,
    /// Never: /system/settings/issue's text, as before.
    Never,
}

/// What is drawn above the words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Art {
    /// Duke, with the name beside him.
    Duke,
    /// The name only.
    Small,
    /// No art: the words only.
    None,
    /// A file's text, as it is.
    File(String),
}

/// welcome.toml, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Welcome {
    pub show: Show,
    pub art: Art,
    pub tagline: String,
    pub version: String,
    pub colors: bool,
}

impl Default for Welcome {
    fn default() -> Welcome {
        Welcome {
            show: Show::EveryBoot,
            art: Art::Duke,
            tagline: TAGLINE.into(),
            version: VERSION.into(),
            colors: true,
        }
    }
}

impl Welcome {
    /// Reads welcome.toml's text: the defaults for what it leaves out, and
    /// what was wrong, each mistake leaving its key at the default.
    pub fn parse(text: &str) -> (Welcome, Vec<String>) {
        let mut welcome = Welcome::default();
        let mut problems = Vec::new();
        let table: toml::Table = match text.parse() {
            Ok(table) => table,
            Err(err) => return (welcome, vec![err.to_string().trim().replace('\n', " ")]),
        };
        for (key, value) in &table {
            match (key.as_str(), value) {
                ("show", toml::Value::String(when)) => match when.as_str() {
                    "every-boot" | "always" => welcome.show = Show::EveryBoot,
                    "first-boot" => welcome.show = Show::FirstBoot,
                    "never" => welcome.show = Show::Never,
                    other => problems.push(format!(
                        "show: '{other}' is not \"every-boot\", \"first-boot\" or \"never\""
                    )),
                },
                ("art", toml::Value::String(art)) => {
                    welcome.art = match art.as_str() {
                        "duke" => Art::Duke,
                        "small" => Art::Small,
                        "none" | "" => Art::None,
                        path if path.starts_with('/') => Art::File(path.to_string()),
                        other => {
                            problems.push(format!(
                                "art: '{other}' is not \"duke\", \"small\", \"none\" or a file's path"
                            ));
                            continue;
                        }
                    };
                }
                ("tagline", toml::Value::String(text)) => welcome.tagline.clone_from(text),
                ("tagline", toml::Value::Boolean(on)) => {
                    welcome.tagline = if *on { TAGLINE.into() } else { String::new() };
                }
                ("version", toml::Value::String(text)) => welcome.version.clone_from(text),
                ("version", toml::Value::Boolean(on)) => {
                    welcome.version = if *on { VERSION.into() } else { String::new() };
                }
                ("colors", toml::Value::Boolean(on)) => welcome.colors = *on,
                ("show" | "art" | "tagline" | "version" | "colors", _) => {
                    problems.push(format!("{key}: not a value it takes (welcome.toml(5))"));
                }
                (other, _) => problems.push(format!("{other}: no such key")),
            }
        }
        (welcome, problems)
    }

    /// The banner's text for a screen `cols` by `rows`, with terminal
    /// escapes for its colours; `expand` gives the version line's escapes
    /// their values. Lines end with `\n`, and a blank line ends it.
    pub fn render(&self, cols: usize, rows: usize, expand: &dyn Fn(&str) -> String) -> String {
        let paint = |sgr: &str, text: &str| {
            if self.colors && !text.is_empty() {
                format!("\x1b[{sgr}m{text}\x1b[0m")
            } else {
                text.to_string()
            }
        };
        let version = expand(&self.version);
        let tagline = self.tagline.clone();
        let mut out = String::new();
        let art = match &self.art {
            Art::File(path) => match std::fs::read_to_string(path) {
                Ok(text) => {
                    out.push_str(&paint("1;33", text.trim_end_matches('\n')));
                    out.push('\n');
                    Art::None
                }
                Err(_) => Art::Duke,
            },
            other => other.clone(),
        };
        // Duke takes 12 lines and 67 columns; the name alone, 4 and 38.
        let art = match art {
            Art::Duke if cols > WIDTH && rows >= DUKE.len() + 6 => Art::Duke,
            Art::Duke | Art::Small if cols > NAME[3].len() && rows >= NAME.len() + 6 => Art::Small,
            Art::File(_) => Art::None,
            _ => Art::None,
        };
        match art {
            Art::Duke => {
                for (row, duke) in DUKE.iter().enumerate() {
                    let beside = match row {
                        2..=5 => paint("1;97", NAME[row - 2]),
                        7 => tagline.clone(),
                        9 => paint("90", &version),
                        _ => String::new(),
                    };
                    let mut line = paint("1;33", duke);
                    if !beside.is_empty() {
                        line.push_str(&" ".repeat(DUKE_WIDTH - duke.len()));
                        line.push_str(&beside);
                    }
                    let _ = writeln!(out, "{line}");
                }
            }
            Art::Small => {
                for name in NAME {
                    let _ = writeln!(out, " {}", paint("1;97", name));
                }
                out.push('\n');
                words(&mut out, &tagline, &paint("90", &version));
            }
            _ => words(&mut out, &tagline, &paint("90", &version)),
        }
        out.push('\n');
        out
    }
}

/// The tagline and the version, each on a line of its own if it has
/// words.
fn words(out: &mut String, tagline: &str, version: &str) {
    for line in [tagline, version] {
        if !line.is_empty() {
            let _ = writeln!(out, " {line}");
        }
    }
}

/// The sign-in screen's lines about the web console for a screen `cols`
/// wide, read from the file at `path`: where to open it, and the
/// certificate's fingerprint, wrapped between its bytes, to compare with
/// the browser's warning. Each line ends with `\n`. Nothing, not even an
/// error, for a file that is missing or is not what admind writes, since
/// a fingerprint that is only half right would mislead.
pub fn web_console(path: &str, cols: usize) -> String {
    std::fs::File::open(path)
        .ok()
        .and_then(|file| {
            let mut text = String::new();
            file.take(WEB_CONSOLE_MAX).read_to_string(&mut text).ok().map(|_| text)
        })
        .and_then(|text| web_console_lines(&text, cols))
        .unwrap_or_default()
}

/// [`web_console`]'s lines for the text of admind's file; `None` if the
/// text is not an `https://` address and a SHA-256 fingerprint.
fn web_console_lines(text: &str, cols: usize) -> Option<String> {
    let mut lines = text.lines();
    let address = lines.next()?;
    let name = address.strip_prefix("https://")?;
    if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_graphic()) {
        return None;
    }
    let fingerprint = lines.next()?.strip_prefix("SHA-256 fingerprint ")?;
    let bytes: Vec<&str> = fingerprint.split(':').collect();
    let hex = |byte: &&str| byte.len() == 2 && byte.bytes().all(|digit| digit.is_ascii_hexdigit());
    if bytes.len() != FINGERPRINT_BYTES || !bytes.iter().all(hex) {
        return None;
    }
    // A line with its one space of indent ends before the last column, so
    // that the cursor never waits to wrap.
    let room = cols.saturating_sub(2);
    let mut out = String::new();
    if "Web console: ".len() + address.len() <= room {
        let _ = writeln!(out, " Web console: {address}");
    } else {
        let _ = writeln!(out, " Web console:\n {address}");
    }
    out.push_str(" Certificate SHA-256 fingerprint:\n");
    // Whole bytes on each line, and the same number on each but the last:
    // 16 and 16 on an 80-column screen, not 26 and 6.
    let fit = ((room + 1) / 3).max(1);
    let per_line = FINGERPRINT_BYTES.div_ceil(FINGERPRINT_BYTES.div_ceil(fit));
    for line in bytes.chunks(per_line) {
        let _ = writeln!(out, " {}", line.join(":"));
    }
    Some(out)
}

/// The web console's file, looked at by `stat` alone (see [`LOOK_OFTEN`]),
/// so that consoled reads it only after admind has written it again.
/// admind replaces the file as a whole, so a write shows in the time the
/// file was modified, or in its length.
pub struct WebConsoleWatch {
    path: String,
    /// The file's modified time and length as the last look found them;
    /// `None` while it is missing.
    seen: Option<(SystemTime, u64)>,
    looked: Instant,
}

impl WebConsoleWatch {
    /// A watch on the file at `path`, which has not been looked at yet.
    pub fn new(path: &str, now: Instant) -> WebConsoleWatch {
        WebConsoleWatch { path: path.to_string(), seen: None, looked: now }
    }

    /// How long after the last look the next is due, when the newest
    /// screen that could be drawn again was drawn at `drawn`.
    fn every(&self, drawn: Instant) -> Duration {
        if self.looked.saturating_duration_since(drawn) < LOOK_OFTEN_FOR {
            LOOK_OFTEN
        } else {
            LOOK_SELDOM
        }
    }

    /// When the next look is due, when the newest screen that could be
    /// drawn again was drawn at `drawn`. With no such screen there is no
    /// deadline, so that consoled sleeps until something else wakes it.
    pub fn deadline(&self, drawn: Option<Instant>) -> Option<Instant> {
        drawn.map(|drawn| self.looked + self.every(drawn))
    }

    /// Looks at the file if a look is due at `now` for screens the newest
    /// of which was drawn at `drawn`, and says whether it was written, or
    /// removed, or made, since the look before.
    pub fn changed(&mut self, now: Instant, drawn: Instant) -> bool {
        if now < self.looked + self.every(drawn) {
            return false;
        }
        self.looked = now;
        let seen = stamp(&self.path);
        if seen == self.seen {
            return false;
        }
        self.seen = seen;
        true
    }
}

/// When the file at `path` was modified and how long it is, or nothing
/// while it is missing.
fn stamp(path: &str) -> Option<(SystemTime, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// Whether the banner shows on this boot: by welcome.toml's `show`, and
/// for `first-boot`, whether `shown` says it has been shown already, which
/// it then records.
pub fn shows_this_boot(welcome: &Welcome, shown: &str) -> bool {
    match welcome.show {
        Show::EveryBoot => true,
        Show::Never => false,
        Show::FirstBoot => {
            if std::fs::metadata(shown).is_ok() {
                return false;
            }
            if let Some(dir) = std::path::Path::new(shown).parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(shown, b"");
            true
        }
    }
}

/// The shipped welcome.toml, which says the defaults.
#[cfg(test)]
const SHIPPED: &str = include_str!("../../../defaults/console/welcome.toml");

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn version(text: &str) -> String {
        text.replace("\\s", "Dukovo").replace("\\r", "0.10.4")
    }

    #[test]
    fn the_shipped_file_says_the_defaults() {
        let (welcome, problems) = Welcome::parse(SHIPPED);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(welcome, Welcome::default());
    }

    #[test]
    fn duke_fits_an_80_column_console() {
        let banner = Welcome::default().render(80, 30, &version);
        let text = plain(&banner);
        assert!(text.lines().all(|line| line.chars().count() <= WIDTH), "{text}");
        assert!(text.lines().all(str::is_ascii), "{text}");
        assert_eq!(text.lines().count(), DUKE.len() + 1);
        assert!(
            text.lines().nth(7).unwrap().ends_with("   A bright future. A human future."),
            "{text}"
        );
        assert!(text.contains("Dukovo Server 0.10.4\n"), "{text}");
        assert!(banner.contains("\x1b[1;33m |\\"), "{banner}");
    }

    #[test]
    fn small_screens_get_less() {
        let text = plain(&Welcome::default().render(60, 30, &version));
        assert!(!text.contains("(__)"), "{text}");
        assert!(text.contains("__| |"), "{text}");
        let text = plain(&Welcome::default().render(30, 10, &version));
        assert_eq!(text, " A bright future. A human future.\n Dukovo Server 0.10.4\n\n");
    }

    #[test]
    fn settings_change_it() {
        let (welcome, problems) = Welcome::parse(
            "show = \"first-boot\"\nart = \"none\"\ntagline = false\ncolors = false\n",
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(welcome.show, Show::FirstBoot);
        assert_eq!(welcome.render(80, 30, &version), " Dukovo Server 0.10.4\n\n");
        let (welcome, problems) = Welcome::parse("show = \"sometimes\"\nart = \"cat\"\nfoo = 1\n");
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert_eq!(welcome, Welcome::default());
        let (_, problems) = Welcome::parse("show = ");
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn first_boot_shows_once() {
        let dir = std::env::temp_dir().join(format!("welcome-test-{}", std::process::id()));
        let shown = dir.join("state/shown");
        let shown = shown.to_str().unwrap();
        let first = Welcome { show: Show::FirstBoot, ..Welcome::default() };
        assert!(shows_this_boot(&first, shown));
        assert!(!shows_this_boot(&first, shown));
        assert!(shows_this_boot(&Welcome::default(), shown));
        let never = Welcome { show: Show::Never, ..Welcome::default() };
        assert!(!shows_this_boot(&never, shown));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// What admind writes: an address, then a fingerprint of bytes 00 to 1f.
    fn issue_file(address: &str) -> String {
        let bytes: Vec<String> = (0..FINGERPRINT_BYTES).map(|byte| format!("{byte:02X}")).collect();
        format!("{address}\nSHA-256 fingerprint {}\n", bytes.join(":"))
    }

    const ADDRESS: &str = "https://duke-nas.local:9090";

    #[test]
    fn the_web_console_is_named_with_its_fingerprint_split_between_bytes() {
        let lines = web_console_lines(&issue_file(ADDRESS), 80).unwrap();
        let first: Vec<String> = (0..16).map(|byte| format!("{byte:02X}")).collect();
        let second: Vec<String> = (16..32).map(|byte| format!("{byte:02X}")).collect();
        assert_eq!(
            lines,
            format!(
                " Web console: {ADDRESS}\n Certificate SHA-256 fingerprint:\n {}\n {}\n",
                first.join(":"),
                second.join(":")
            )
        );
    }

    #[test]
    fn a_wide_screen_gets_the_fingerprint_on_one_line() {
        let lines = web_console_lines(&issue_file(ADDRESS), 100).unwrap();
        assert_eq!(lines.lines().count(), 3, "{lines}");
        assert_eq!(lines.lines().last().unwrap().len(), 1 + 32 * 3 - 1);
    }

    #[test]
    fn a_narrow_screen_gets_shorter_lines_that_all_fit() {
        let all: Vec<String> = (0..FINGERPRINT_BYTES).map(|byte| format!("{byte:02X}")).collect();
        for cols in [40, 41, 50, 64, 80, 95, 96, 97, 132] {
            let lines = web_console_lines(&issue_file(ADDRESS), cols).unwrap();
            for line in lines.lines() {
                assert!(line.len() < cols, "{cols} columns: {line:?}");
            }
            // Every byte is there once, in order, whatever the wrapping.
            let label = lines.lines().position(|line| line.ends_with("fingerprint:")).unwrap();
            let fingerprint: Vec<&str> = lines.lines().skip(label + 1).map(str::trim).collect();
            assert_eq!(fingerprint.join(":"), all.join(":"), "{cols} columns");
        }
        // 32 bytes in three lines of 11, 11 and 10 on the smallest console.
        let lines = web_console_lines(&issue_file(ADDRESS), 40).unwrap();
        let sizes: Vec<usize> = lines.lines().skip(3).map(|line| line.split(':').count()).collect();
        assert_eq!(sizes, [11, 11, 10], "{lines}");
    }

    #[test]
    fn a_long_address_goes_on_a_line_of_its_own() {
        let address = "https://a-name-that-is-long.example.org:9090";
        let lines = web_console_lines(&issue_file(address), 50).unwrap();
        assert!(lines.starts_with(&format!(" Web console:\n {address}\n")), "{lines}");
        let lines = web_console_lines(&issue_file(address), 80).unwrap();
        assert!(lines.starts_with(&format!(" Web console: {address}\n")), "{lines}");
    }

    #[test]
    fn a_file_that_is_not_what_admind_writes_shows_nothing() {
        let good = issue_file(ADDRESS);
        let bytes: Vec<String> = (0..FINGERPRINT_BYTES).map(|byte| format!("{byte:02X}")).collect();
        let short = bytes[1..].join(":");
        let nothing = [
            String::new(),
            format!("{ADDRESS}\n"),
            good.replace("https://", "http://"),
            good.replace("https://", "https:// "),
            good.replace("https://duke-nas.local:9090", "https://"),
            good.replace("SHA-256 fingerprint", "MD5 fingerprint"),
            good.replace("00:", "0G:"),
            good.replace(":", " "),
            format!("{ADDRESS}\nSHA-256 fingerprint {short}\n"),
            format!("{ADDRESS}\nSHA-256 fingerprint {}:00\n", bytes.join(":")),
            good.replace("duke-nas", "duke\x1b[2Jnas"),
            good.replace("duke-nas", "d\u{fc}ke"),
        ];
        for text in &nothing {
            assert_eq!(web_console_lines(text, 80), None, "{text:?}");
        }
        // Lines after the two are not read.
        assert!(web_console_lines(&format!("{good}something else\n"), 80).is_some());
    }

    #[test]
    fn the_web_console_is_read_from_the_file_each_time() {
        let dir = std::env::temp_dir().join(format!("web-console-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("issue");
        let path = path.to_str().unwrap();
        assert_eq!(web_console(path, 80), "", "no file");
        std::fs::write(path, issue_file(ADDRESS)).unwrap();
        assert!(web_console(path, 80).starts_with(" Web console: https://duke-nas.local:9090\n"));
        std::fs::write(path, issue_file("https://other.local:9090")).unwrap();
        assert!(web_console(path, 80).starts_with(" Web console: https://other.local:9090\n"));
        // A file that is not text shows nothing, and so does one whose
        // address runs past what is read of it.
        std::fs::write(path, [0xff, 0xfe, b'\n']).unwrap();
        assert_eq!(web_console(path, 80), "");
        let long = issue_file(&format!("https://{}", "a".repeat(2000)));
        std::fs::write(path, long).unwrap();
        assert_eq!(web_console(path, 80), "");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_file_is_looked_at_every_two_seconds_after_a_screen_is_drawn() {
        let dir = std::env::temp_dir().join(format!("web-console-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("issue");
        let path = path.to_str().unwrap();
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut watch = WebConsoleWatch::new(path, start);
        std::fs::write(path, "one").unwrap();
        assert!(!watch.changed(at(1), start), "a look is not due");
        assert!(watch.changed(at(2), start), "the file was made");
        std::fs::write(path, "other").unwrap();
        assert!(!watch.changed(at(3), start), "a look is not due");
        assert!(watch.changed(at(4), start), "the file was written again");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_that_is_as_it_was_is_not_a_change() {
        let dir = std::env::temp_dir().join(format!("web-console-same-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("issue");
        let path = path.to_str().unwrap();
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut watch = WebConsoleWatch::new(path, start);
        assert!(!watch.changed(at(2), start), "a file that is missing is as it was");
        std::fs::write(path, "one").unwrap();
        assert!(watch.changed(at(4), start));
        assert!(!watch.changed(at(6), start), "unchanged: nothing to read");
        assert!(!watch.changed(at(8), start), "unchanged: nothing to read");
        std::fs::write(path, "longer").unwrap();
        assert!(watch.changed(at(10), start));
        std::fs::remove_file(path).unwrap();
        assert!(watch.changed(at(12), start), "admind removes the file when it stops");
        assert!(!watch.changed(at(14), start));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn five_minutes_after_the_newest_screen_the_file_is_looked_at_once_a_minute() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut watch = WebConsoleWatch::new("/nowhere", start);
        assert!(!watch.changed(at(300), start));
        assert_eq!(watch.deadline(Some(start)), Some(at(360)));
        assert!(!watch.changed(at(359), start), "a look is not due");
        assert!(!watch.changed(at(360), start), "a look, and nothing changed");
        // A screen drawn now is looked for every two seconds again.
        assert_eq!(watch.deadline(Some(at(361))), Some(at(362)));
    }

    #[test]
    fn nothing_wakes_consoled_while_no_screen_waits_for_the_file() {
        let start = Instant::now();
        let watch = WebConsoleWatch::new("/nowhere", start);
        assert_eq!(watch.deadline(None), None);
        assert_eq!(watch.deadline(Some(start)), Some(start + LOOK_OFTEN));
    }
}
