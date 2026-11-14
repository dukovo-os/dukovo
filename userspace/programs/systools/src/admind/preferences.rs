// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A person's preferences of the web console (ADR 0516): the colours, the
//! size of the text, whether things move, and the time zone the console
//! shows times in. Each is one top-level key of the person's own
//! `~/.config/dukovo-admin/settings.toml`, named as the API names it:
//!
//! - `theme`: `dark` (the default), `light`, `high-contrast` or `device`,
//!   which follows the device's own;
//! - `text`: the size of the text in percent, 100 (the default), 112, 125
//!   or 150;
//! - `motion`: `full` (the default) or `less`;
//! - `zone`: a time zone of the machine's database, such as
//!   `Europe/Berlin`, or empty (the default) while the person has none;
//! - `commands`: `hidden` (the default) or `shown`, whether pages show the
//!   command that an action runs;
//! - the `[terminal]` table: `size` of the text in pixels, 12 to 20 (14);
//!   `scrollback` in lines, 1000 to 50000 (10000); `copy-on-select`
//!   (false); `keep-days` that a terminal that no tab is attached to is
//!   kept, 1 to 30 (7); `announce`, to read new output aloud (false); and
//!   `shortcuts`, the terminal's Alt+Shift keys (true). The API names them
//!   the same, under `terminal`.
//!
//! The person's own worker reads and writes the file, as them, through
//! [`settings_edit`], so that a comment they wrote stays and a change that
//! changes nothing writes nothing. The file is theirs to edit by hand: a
//! key that is not there, or has a value that is none of the above, reads
//! as its default, so that a typo costs a person a preference and never
//! the page; a file that is not TOML is told, with its line, and left as
//! it is. What the page sends is checked by [`Change::check`] before the
//! worker is asked, and by the worker again before it writes.

use std::convert::Infallible;
use std::ffi::OsString;
use std::fs::DirBuilder;
use std::ops::RangeInclusive;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item};

use super::errors::{Failure, Kind};
use super::link::Fault;
use crate::settings_edit;
use crate::timedatectl;
use crate::util::strip_os_error;

/// The themes the console has, the default first.
pub const THEMES: [&str; 4] = ["dark", "light", "high-contrast", "device"];

/// The sizes of the text, in percent, the default first.
pub const TEXT_SIZES: [i64; 4] = [100, 112, 125, 150];

/// The ways things may move, the default first.
pub const MOTIONS: [&str; 2] = ["full", "less"];

/// Whether pages show the command an action runs, the default first.
pub const COMMANDS: [&str; 2] = ["hidden", "shown"];

/// What the terminal's text may be, in pixels, and what it is.
pub const TERMINAL_SIZES: RangeInclusive<i64> = 12..=20;
pub const TERMINAL_SIZE: i64 = 14;

/// How many lines the browser keeps of a terminal, and what it keeps.
pub const TERMINAL_SCROLLBACKS: RangeInclusive<i64> = 1_000..=50_000;
pub const TERMINAL_SCROLLBACK: i64 = 10_000;

/// How many days a terminal that no tab is attached to is kept, and how
/// many it is when the person has not said.
pub const TERMINAL_KEEP_DAYS: RangeInclusive<i64> = 1..=30;
pub const TERMINAL_KEEP: i64 = 7;

/// The person's folder for the console's files, in their home, and the
/// file in it.
const FOLDER: &str = ".config/dukovo-admin";
const FILE: &str = "settings.toml";

/// The mode of the folder, when it is made, and of the file.
const FOLDER_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

/// Everything a person prefers, as the page reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Preferences {
    /// One of [`THEMES`].
    pub theme: String,
    /// One of [`TEXT_SIZES`].
    pub text: i64,
    /// One of [`MOTIONS`].
    pub motion: String,
    /// A time zone, or empty.
    pub zone: String,
    /// One of [`COMMANDS`].
    pub commands: String,
    /// The terminal's.
    pub terminal: Terminal,
}

impl Default for Preferences {
    fn default() -> Preferences {
        Preferences {
            theme: THEMES[0].into(),
            text: TEXT_SIZES[0],
            motion: MOTIONS[0].into(),
            zone: String::new(),
            commands: COMMANDS[0].into(),
            terminal: Terminal::default(),
        }
    }
}

/// What a person prefers of the terminal: the `[terminal]` table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Terminal {
    /// The text's size in pixels, in [`TERMINAL_SIZES`].
    pub size: i64,
    /// The lines the browser keeps, in [`TERMINAL_SCROLLBACKS`].
    pub scrollback: i64,
    /// Whether a selection is copied as soon as it is made.
    #[serde(rename = "copy-on-select")]
    pub copy_on_select: bool,
    /// The days a terminal that no tab is attached to is kept, in
    /// [`TERMINAL_KEEP_DAYS`].
    #[serde(rename = "keep-days")]
    pub keep_days: i64,
    /// Whether new output is read aloud.
    pub announce: bool,
    /// Whether the terminal's Alt+Shift keys are on.
    pub shortcuts: bool,
}

impl Default for Terminal {
    fn default() -> Terminal {
        Terminal {
            size: TERMINAL_SIZE,
            scrollback: TERMINAL_SCROLLBACK,
            copy_on_select: false,
            keep_days: TERMINAL_KEEP,
            announce: false,
            shortcuts: true,
        }
    }
}

impl Terminal {
    /// What the table `terminal` of a settings file says, each key that is
    /// not there or is not right reading as its default. A `terminal` that
    /// is no table reads as the defaults.
    fn of(terminal: Option<&Item>) -> Terminal {
        let default = Terminal::default();
        let Some(table) = terminal.and_then(Item::as_table_like) else {
            return default;
        };
        let number = |key: &str, range: &RangeInclusive<i64>, default: i64| {
            table
                .get(key)
                .and_then(Item::as_integer)
                .filter(|value| range.contains(value))
                .unwrap_or(default)
        };
        let switch =
            |key: &str, default: bool| table.get(key).and_then(Item::as_bool).unwrap_or(default);
        Terminal {
            size: number("size", &TERMINAL_SIZES, default.size),
            scrollback: number("scrollback", &TERMINAL_SCROLLBACKS, default.scrollback),
            copy_on_select: switch("copy-on-select", default.copy_on_select),
            keep_days: number("keep-days", &TERMINAL_KEEP_DAYS, default.keep_days),
            announce: switch("announce", default.announce),
            shortcuts: switch("shortcuts", default.shortcuts),
        }
    }
}

impl Preferences {
    /// What `document` says, a key that is not there or is not right
    /// reading as its default. `is_zone` says which zones are the
    /// machine's.
    pub(crate) fn of(document: &DocumentMut, is_zone: impl Fn(&str) -> bool) -> Preferences {
        let default = Preferences::default();
        let word = |key: &str, allowed: &[&str], default: String| {
            document
                .get(key)
                .and_then(Item::as_str)
                .filter(|value| allowed.contains(value))
                .map_or(default, str::to_string)
        };
        let text = document
            .get("text")
            .and_then(Item::as_integer)
            .filter(|size| TEXT_SIZES.contains(size))
            .unwrap_or(default.text);
        let zone = document
            .get("zone")
            .and_then(Item::as_str)
            .filter(|zone| is_zone(zone))
            .map_or(default.zone, str::to_string);
        Preferences {
            theme: word("theme", &THEMES, default.theme),
            text,
            motion: word("motion", &MOTIONS, default.motion),
            zone,
            commands: word("commands", &COMMANDS, default.commands),
            terminal: Terminal::of(document.get("terminal")),
        }
    }
}

/// What a `POST` changes: any of the preferences, and nothing else.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Change {
    /// One of [`THEMES`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// One of [`TEXT_SIZES`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<i64>,
    /// One of [`MOTIONS`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion: Option<String>,
    /// A time zone, or empty to have none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
    /// One of [`COMMANDS`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands: Option<String>,
    /// Any of the terminal's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<TerminalChange>,
}

/// What a `POST` changes of the terminal's preferences: any of them.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalChange {
    /// In [`TERMINAL_SIZES`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    /// In [`TERMINAL_SCROLLBACKS`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrollback: Option<i64>,
    #[serde(rename = "copy-on-select", skip_serializing_if = "Option::is_none")]
    pub copy_on_select: Option<bool>,
    /// In [`TERMINAL_KEEP_DAYS`].
    #[serde(rename = "keep-days", skip_serializing_if = "Option::is_none")]
    pub keep_days: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub announce: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcuts: Option<bool>,
}

impl TerminalChange {
    /// Whether it changes nothing.
    fn is_empty(&self) -> bool {
        *self == TerminalChange::default()
    }

    /// Checks each number it holds against its range.
    fn check(&self) -> Result<(), Failure> {
        let ranges = [
            ("terminal.size", self.size, &TERMINAL_SIZES, "the terminal's text is {} to {} pixels"),
            (
                "terminal.scrollback",
                self.scrollback,
                &TERMINAL_SCROLLBACKS,
                "the terminal keeps {} to {} lines",
            ),
            (
                "terminal.keep-days",
                self.keep_days,
                &TERMINAL_KEEP_DAYS,
                "a terminal is kept for {} to {} days",
            ),
        ];
        for (field, value, range, words) in ranges {
            if let Some(value) = value
                && !range.contains(&value)
            {
                let why = words.replacen("{}", &range.start().to_string(), 1).replacen(
                    "{}",
                    &range.end().to_string(),
                    1,
                );
                return Err(Failure::bad_value(field, why));
            }
        }
        Ok(())
    }

    /// Puts what it holds into the table `terminal` of `document`, which
    /// is made when it is not there and made over when it is no table.
    fn apply(&self, document: &mut DocumentMut) {
        if self.is_empty() {
            return;
        }
        let table = match settings_edit::table(document, "terminal") {
            Some(table) => table,
            None => {
                document.remove("terminal");
                settings_edit::table(document, "terminal").expect("a table was just made")
            }
        };
        if let Some(size) = self.size {
            settings_edit::set_integer(table, "size", size);
        }
        if let Some(scrollback) = self.scrollback {
            settings_edit::set_integer(table, "scrollback", scrollback);
        }
        if let Some(copy) = self.copy_on_select {
            settings_edit::set_bool(table, "copy-on-select", copy);
        }
        if let Some(days) = self.keep_days {
            settings_edit::set_integer(table, "keep-days", days);
        }
        if let Some(announce) = self.announce {
            settings_edit::set_bool(table, "announce", announce);
        }
        if let Some(shortcuts) = self.shortcuts {
            settings_edit::set_bool(table, "shortcuts", shortcuts);
        }
    }
}

/// `words` as a sentence's list: `a, b or c`.
fn either(words: &[impl ToString]) -> String {
    let words: Vec<String> = words.iter().map(ToString::to_string).collect();
    match words.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

impl Change {
    /// Whether it changes nothing: the page only reads.
    pub fn is_empty(&self) -> bool {
        let Change { theme, text, motion, zone, commands, terminal } = self;
        theme.is_none()
            && text.is_none()
            && motion.is_none()
            && zone.is_none()
            && commands.is_none()
            && terminal.as_ref().is_none_or(TerminalChange::is_empty)
    }

    /// Checks each value it holds against what the preference may be;
    /// `is_zone` says which zones are the machine's. The words say what
    /// would work, and never repeat the value, which a browser chose.
    ///
    /// # Errors
    ///
    /// The first value that cannot work: [`Kind::BadValue`] with the name
    /// of its key.
    pub fn check(&self, is_zone: impl Fn(&str) -> bool) -> Result<(), Failure> {
        if let Some(theme) = &self.theme
            && !THEMES.contains(&theme.as_str())
        {
            return Err(Failure::bad_value("theme", format!("a theme is {}", either(&THEMES))));
        }
        if let Some(text) = self.text
            && !TEXT_SIZES.contains(&text)
        {
            let sizes = either(&TEXT_SIZES);
            return Err(Failure::bad_value("text", format!("a text size is {sizes} percent")));
        }
        if let Some(motion) = &self.motion
            && !MOTIONS.contains(&motion.as_str())
        {
            return Err(Failure::bad_value("motion", format!("motion is {}", either(&MOTIONS))));
        }
        if let Some(zone) = &self.zone
            && !zone.is_empty()
            && !is_zone(zone)
        {
            let why = "a zone is empty, or a time zone of this machine, such as Europe/Berlin";
            return Err(Failure::bad_value("zone", why));
        }
        if let Some(commands) = &self.commands
            && !COMMANDS.contains(&commands.as_str())
        {
            let why = format!("commands are {}", either(&COMMANDS));
            return Err(Failure::bad_value("commands", why));
        }
        if let Some(terminal) = &self.terminal {
            terminal.check()?;
        }
        Ok(())
    }

    /// Puts what it holds into `document`, where the file keeps it.
    pub(crate) fn apply(&self, document: &mut DocumentMut) {
        let table = document.as_table_mut();
        if let Some(theme) = &self.theme {
            settings_edit::set_string(table, "theme", theme);
        }
        if let Some(text) = self.text {
            settings_edit::set_integer(table, "text", text);
        }
        if let Some(motion) = &self.motion {
            settings_edit::set_string(table, "motion", motion);
        }
        if let Some(zone) = &self.zone {
            settings_edit::set_string(table, "zone", zone);
        }
        if let Some(commands) = &self.commands {
            settings_edit::set_string(table, "commands", commands);
        }
        if let Some(terminal) = &self.terminal {
            terminal.apply(document);
        }
    }
}

/// The home folder that the worker's `HOME` names, which authd sets from
/// the account: a path from the root.
///
/// # Errors
///
/// In plain words, when there is none or it does not start at the root.
pub fn home_of(variable: Option<OsString>) -> Result<PathBuf, String> {
    match variable.map(PathBuf::from) {
        Some(home) if home.is_absolute() => Ok(home),
        Some(_) => Err("the worker's home folder is not a path from the root".into()),
        None => Err("the worker has no home folder".into()),
    }
}

/// The preferences in the file of the person whose home is `home`, once
/// `change` is made, which writes nothing when it changes nothing. The
/// folder is made 0700 when it is not there, and the file 0600.
fn after(
    home: &Path,
    change: &Change,
    is_zone: impl Fn(&str) -> bool,
) -> Result<Preferences, Fault> {
    let folder = home.join(FOLDER);
    // A change that is not empty writes, and a file that is not there
    // needs its folder; one that only reads makes nothing.
    if !change.is_empty() {
        DirBuilder::new().recursive(true).mode(FOLDER_MODE).create(&folder).map_err(|error| {
            let why = format!("cannot make {}: {}", folder.display(), strip_os_error(&error));
            Fault::new(Kind::Internal, why)
        })?;
    }
    let mut now = Preferences::default();
    settings_edit::change(&folder.join(FILE), FILE_MODE, |document| {
        change.apply(document);
        now = Preferences::of(document, &is_zone);
        Ok::<(), Infallible>(())
    })
    .map_err(|changed| Fault::new(Kind::Internal, changed.to_string()))?;
    Ok(now)
}

/// The preferences of the person whose home is `home`.
///
/// # Errors
///
/// A fault of [`Kind::Internal`] that says why the file cannot be read.
pub fn read(home: &Path) -> Result<Preferences, Fault> {
    after(home, &Change::default(), timedatectl::is_zone)
}

/// Makes `change` to the preferences of the person whose home is `home`,
/// and gives them all.
///
/// # Errors
///
/// A fault of [`Kind::BadValue`] for a value that cannot work, which the
/// worker will not have been sent, and of [`Kind::Internal`] that says why
/// the file cannot be read or written.
pub fn change(home: &Path, asked: &Change) -> Result<Preferences, Fault> {
    change_with(home, asked, timedatectl::is_zone)
}

/// [`change`] for the zones `is_zone` calls the machine's.
fn change_with(
    home: &Path,
    asked: &Change,
    is_zone: impl Fn(&str) -> bool,
) -> Result<Preferences, Fault> {
    asked.check(&is_zone).map_err(|failure| Fault::new(failure.kind, failure.detail))?;
    after(home, asked, is_zone)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use serde_json::{Value, json};

    use super::*;

    /// A home folder of its own under the system's temporary folder,
    /// taken away at the end.
    struct Home(PathBuf);

    impl Home {
        fn new(name: &str) -> Home {
            let path = std::env::temp_dir()
                .join(format!("admind-preferences-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Home(path)
        }

        /// The person's file, which is not there until something makes it.
        fn file(&self) -> PathBuf {
            self.0.join(FOLDER).join(FILE)
        }

        /// Makes the file hold `text`, as a person would by hand.
        fn write(&self, text: &str) {
            fs::create_dir_all(self.file().parent().unwrap()).unwrap();
            fs::write(self.file(), text).unwrap();
        }

        fn text(&self) -> String {
            fs::read_to_string(self.file()).unwrap()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The zones of the machine the tests pretend to be.
    fn zones(zone: &str) -> bool {
        ["Europe/Berlin", "Asia/Calcutta", "UTC"].contains(&zone)
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().mode() & 0o7777
    }

    #[test]
    fn a_person_with_no_file_has_the_defaults_and_reading_makes_nothing() {
        let home = Home::new("none");
        let found = after(&home.0, &Change::default(), zones).unwrap();
        assert_eq!(found, Preferences::default());
        assert_eq!(
            json!(found),
            json!({
                "theme": "dark", "text": 100, "motion": "full", "zone": "", "commands": "hidden",
                "terminal": {
                    "size": 14, "scrollback": 10000, "copy-on-select": false, "keep-days": 7,
                    "announce": false, "shortcuts": true,
                },
            })
        );
        assert!(!home.0.join(".config").exists());
        assert_eq!(read(&home.0).unwrap(), Preferences::default());
    }

    #[test]
    fn a_file_is_read_key_by_key_and_what_is_not_right_reads_as_the_default() {
        let home = Home::new("read");
        home.write(
            "theme = \"high-contrast\"\ntext = 150\nmotion = \"less\"\nzone = \"Asia/Calcutta\"\n",
        );
        let found = after(&home.0, &Change::default(), zones).unwrap();
        let all = ("high-contrast", 150, "less", "Asia/Calcutta");
        assert_eq!((found.theme.as_str(), found.text, found.motion.as_str(), &*found.zone), all);
        // A key that is not there is the default.
        home.write("theme = \"light\"\n");
        let found = after(&home.0, &Change::default(), zones).unwrap();
        assert_eq!(found, Preferences { theme: "light".into(), ..Preferences::default() });
        // A value that is none of the preference's, or of another type,
        // and a zone the machine does not have, are the default too.
        for odd in [
            "theme = \"purple\"\ntext = 130\nmotion = \"none\"\nzone = \"Mars/Base\"\n",
            "theme = 3\ntext = \"125\"\nmotion = true\nzone = 1\n",
            "theme = \"Light\"\ntext = 112.0\nmotion = \"\"\nzone = \"europe/berlin\"\n",
            "theme = [\"light\"]\n[text]\nsize = 125\n",
        ] {
            home.write(odd);
            let found = after(&home.0, &Change::default(), zones).unwrap();
            assert_eq!(found, Preferences::default(), "{odd}");
        }
        // Reading changed none of them.
        assert!(home.text().contains("[text]"));
    }

    #[test]
    fn a_file_that_is_not_toml_is_told_with_its_line_and_is_left_as_it_is() {
        let home = Home::new("broken");
        let broken = "theme = \"light\"\nthis is not toml\n";
        home.write(broken);
        let change = Change { theme: Some("dark".into()), ..Change::default() };
        for fault in [
            after(&home.0, &Change::default(), zones).unwrap_err(),
            after(&home.0, &change, zones).unwrap_err(),
        ] {
            assert_eq!(fault.id, "ADM-121");
            let path = home.file();
            assert!(
                fault
                    .detail
                    .starts_with(&format!("{} is not valid TOML, at line 2: ", path.display())),
                "{}",
                fault.detail
            );
        }
        assert_eq!(home.text(), broken);
    }

    #[test]
    fn the_first_change_makes_the_folders_0700_and_the_file_0600_and_gives_every_preference() {
        let home = Home::new("first");
        let change = Change { theme: Some("light".into()), ..Change::default() };
        let found = after(&home.0, &change, zones).unwrap();
        assert_eq!(found, Preferences { theme: "light".into(), ..Preferences::default() });
        assert_eq!(home.text(), "theme = \"light\"\n");
        assert_eq!(mode_of(&home.file()), 0o600);
        assert_eq!(mode_of(home.file().parent().unwrap()), 0o700);
        assert_eq!(mode_of(&home.0.join(".config")), 0o700);
        // The folder that was there is not made over.
        fs::set_permissions(home.file().parent().unwrap(), fs::Permissions::from_mode(0o750))
            .unwrap();
        after(&home.0, &Change { text: Some(112), ..Change::default() }, zones).unwrap();
        assert_eq!(mode_of(home.file().parent().unwrap()), 0o750);
        assert_eq!(home.text(), "theme = \"light\"\ntext = 112\n");
    }

    #[test]
    fn a_change_keeps_the_comments_the_blank_lines_and_the_order_of_a_file_made_by_hand() {
        let home = Home::new("by-hand");
        home.write(
            "# My settings.\n\ntheme = \"light\" # for the day\n\n# The size of the text.\n\
             text = 112\n",
        );
        let change = Change {
            theme: Some("dark".into()),
            text: Some(112),
            zone: Some("Europe/Berlin".into()),
            ..Change::default()
        };
        let found = after(&home.0, &change, zones).unwrap();
        assert_eq!(
            home.text(),
            "# My settings.\n\ntheme = \"dark\" # for the day\n\n# The size of the text.\n\
             text = 112\nzone = \"Europe/Berlin\"\n"
        );
        let all = ("dark", 112, "full", "Europe/Berlin");
        assert_eq!((found.theme.as_str(), found.text, found.motion.as_str(), &*found.zone), all);
    }

    #[test]
    fn a_change_to_what_the_file_says_already_writes_nothing() {
        let home = Home::new("same");
        home.write("# Mine.\ntheme = 'light'\ntext = 125 # big\n");
        let before = fs::metadata(home.file()).unwrap().ino();
        let same = Change { theme: Some("light".into()), text: Some(125), ..Change::default() };
        for change in [same, Change::default()] {
            after(&home.0, &change, zones).unwrap();
            assert_eq!(home.text(), "# Mine.\ntheme = 'light'\ntext = 125 # big\n");
            assert_eq!(fs::metadata(home.file()).unwrap().ino(), before);
        }
        // A key that is not there is written, even as what it would read as.
        let default = Change { motion: Some("full".into()), ..Change::default() };
        after(&home.0, &default, zones).unwrap();
        assert!(home.text().ends_with("text = 125 # big\nmotion = \"full\"\n"), "{}", home.text());
    }

    #[test]
    fn the_worker_checks_a_change_again_and_writes_nothing_that_cannot_work() {
        let home = Home::new("again");
        let wrong = Change { zone: Some("Mars/Base".into()), ..Change::default() };
        let fault = change_with(&home.0, &wrong, zones).unwrap_err();
        assert_eq!(fault.id, "ADM-125");
        assert!(!home.0.join(".config").exists());
        // The machine's own zones are used when none is given: this
        // machine has no Mars/Base, and an empty zone is always fine.
        assert_eq!(change(&home.0, &wrong).unwrap_err().id, "ADM-125");
        let none = Change { zone: Some(String::new()), ..Change::default() };
        assert_eq!(change(&home.0, &none).unwrap().zone, "");
        assert_eq!(home.text(), "zone = \"\"\n");
    }

    #[test]
    fn each_value_is_checked_against_what_the_preference_may_be_and_never_repeated() {
        let good = Change {
            theme: Some("high-contrast".into()),
            text: Some(150),
            motion: Some("less".into()),
            zone: Some("Asia/Calcutta".into()),
            commands: Some("shown".into()),
            terminal: Some(TerminalChange {
                size: Some(20),
                scrollback: Some(1_000),
                keep_days: Some(30),
                ..TerminalChange::default()
            }),
        };
        assert!(good.check(zones).is_ok());
        assert!(Change::default().check(zones).is_ok());
        assert!(Change { zone: Some(String::new()), ..Change::default() }.check(zones).is_ok());
        for theme in THEMES {
            assert!(Change { theme: Some(theme.into()), ..Change::default() }.check(zones).is_ok());
        }
        for text in TEXT_SIZES {
            assert!(Change { text: Some(text), ..Change::default() }.check(zones).is_ok());
        }
        let secret = "hunter2";
        let cases = [
            (Change { theme: Some(secret.into()), ..Change::default() }, "theme"),
            (Change { theme: Some("Dark".into()), ..Change::default() }, "theme"),
            (Change { theme: Some(String::new()), ..Change::default() }, "theme"),
            (Change { text: Some(130), ..Change::default() }, "text"),
            (Change { text: Some(0), ..Change::default() }, "text"),
            (Change { text: Some(-100), ..Change::default() }, "text"),
            (Change { motion: Some(secret.into()), ..Change::default() }, "motion"),
            (Change { zone: Some(secret.into()), ..Change::default() }, "zone"),
            (Change { zone: Some("../etc/passwd".into()), ..Change::default() }, "zone"),
            (Change { zone: Some("Europe/Berlin\0".into()), ..Change::default() }, "zone"),
        ];
        for (change, field) in cases {
            let failure = change.check(zones).unwrap_err();
            assert_eq!(failure.kind, Kind::BadValue, "{change:?}");
            assert_eq!(failure.field, Some(field), "{change:?}");
            assert!(!failure.detail.contains(secret), "{}", failure.detail);
        }
        // The first that cannot work is the one told.
        let two = Change { theme: Some("x".into()), motion: Some("y".into()), ..Change::default() };
        assert_eq!(two.check(zones).unwrap_err().field, Some("theme"));
    }

    #[test]
    fn the_words_of_a_refusal_name_what_would_work() {
        let said = |change: Change| change.check(zones).unwrap_err().detail;
        let theme = Change { theme: Some("x".into()), ..Change::default() };
        let text = Change { text: Some(1), ..Change::default() };
        let motion = Change { motion: Some("x".into()), ..Change::default() };
        let zone = Change { zone: Some("x".into()), ..Change::default() };
        assert_eq!(said(theme), "a theme is dark, light, high-contrast or device");
        assert_eq!(said(text), "a text size is 100, 112, 125 or 150 percent");
        assert_eq!(said(motion), "motion is full or less");
        assert!(said(zone).starts_with("a zone is empty, or a time zone of this machine"));
        let commands = Change { commands: Some("x".into()), ..Change::default() };
        assert_eq!(said(commands), "commands are hidden or shown");
        let terminal =
            |change: TerminalChange| Change { terminal: Some(change), ..Change::default() };
        let size = TerminalChange { size: Some(1), ..TerminalChange::default() };
        let scrollback = TerminalChange { scrollback: Some(1), ..TerminalChange::default() };
        let keep = TerminalChange { keep_days: Some(1_000), ..TerminalChange::default() };
        assert_eq!(said(terminal(size)), "the terminal's text is 12 to 20 pixels");
        assert_eq!(said(terminal(scrollback)), "the terminal keeps 1000 to 50000 lines");
        assert_eq!(said(terminal(keep)), "a terminal is kept for 1 to 30 days");
        let nothing: [&str; 0] = [];
        assert_eq!(either(&nothing), "");
        assert_eq!(either(&["one"]), "one");
    }

    #[test]
    fn a_change_reads_from_json_with_any_of_the_keys_and_no_others() {
        let read = |value: Value| serde_json::from_value::<Change>(value);
        assert!(read(json!({})).unwrap().is_empty());
        let all = read(json!({ "theme": "light", "text": 112, "motion": "less", "zone": "UTC" }));
        assert_eq!(
            all.unwrap(),
            Change {
                theme: Some("light".into()),
                text: Some(112),
                motion: Some("less".into()),
                zone: Some("UTC".into()),
                ..Change::default()
            }
        );
        for odd in [
            json!({ "themes": "light" }),
            json!({ "text": "112" }),
            json!({ "text": 112.5 }),
            json!({ "theme": 1 }),
            json!({ "zone": null, "extra": 1 }),
            json!("light"),
        ] {
            assert!(read(odd.clone()).is_err(), "{odd}");
        }
        // A key that is null is a key that is not there.
        assert!(read(json!({ "zone": null })).unwrap().is_empty());
        // What goes to the worker is only what was given.
        let some = Change { zone: Some(String::new()), text: Some(125), ..Change::default() };
        assert_eq!(json!(some), json!({ "text": 125, "zone": "" }));
        assert_eq!(json!(Change::default()), json!({}));
    }

    /// A change of the terminal's keys only.
    fn terminal(change: TerminalChange) -> Change {
        Change { terminal: Some(change), ..Change::default() }
    }

    #[test]
    fn the_defaults_of_the_commands_and_the_terminal_are_what_the_design_gives() {
        let found = Preferences::default();
        assert_eq!(found.commands, "hidden");
        let terminal = found.terminal;
        assert_eq!((terminal.size, terminal.scrollback, terminal.keep_days), (14, 10_000, 7));
        assert_eq!(
            (terminal.copy_on_select, terminal.announce, terminal.shortcuts),
            (false, false, true)
        );
    }

    #[test]
    fn the_commands_and_the_terminals_keys_are_read_one_by_one() {
        let home = Home::new("terminal-read");
        let read = |text: &str| {
            home.write(text);
            after(&home.0, &Change::default(), zones).unwrap()
        };
        let found = read(
            "commands = \"shown\"\n\n[terminal]\nsize = 18\nscrollback = 2000\n\
             copy-on-select = true\nkeep-days = 30\nannounce = true\nshortcuts = false\n",
        );
        assert_eq!(found.commands, "shown");
        assert_eq!(
            found.terminal,
            Terminal {
                size: 18,
                scrollback: 2_000,
                copy_on_select: true,
                keep_days: 30,
                announce: true,
                shortcuts: false,
            }
        );
        // A key that is not there is its own default, and so is a table.
        let found = read("[terminal]\nsize = 16\n");
        let default = Terminal::default();
        assert_eq!(found.terminal, Terminal { size: 16, ..default.clone() });
        assert_eq!(found.commands, "hidden");
        assert_eq!(read("theme = \"light\"\n").terminal, default);
        // A table written inline or with dotted keys is read as a table.
        assert_eq!(read("terminal = { size = 12, shortcuts = false }\n").terminal.size, 12);
        assert_eq!(read("terminal.keep-days = 3\n").terminal.keep_days, 3);
    }

    #[test]
    fn a_wrong_value_of_the_commands_or_the_terminal_reads_as_its_default_and_the_rest_stay() {
        let home = Home::new("terminal-odd");
        let default = Terminal::default();
        let read = |text: &str| {
            home.write(text);
            after(&home.0, &Change::default(), zones).unwrap()
        };
        // At the limits of each number, and one past them.
        for (key, low, high, default) in [
            ("size", 12, 20, default.size),
            ("scrollback", 1_000, 50_000, default.scrollback),
            ("keep-days", 1, 30, default.keep_days),
        ] {
            let at = |value: i64| {
                let found = read(&format!("[terminal]\n{key} = {value}\n")).terminal;
                match key {
                    "size" => found.size,
                    "scrollback" => found.scrollback,
                    _ => found.keep_days,
                }
            };
            assert_eq!((at(low), at(high)), (low, high), "{key}");
            assert_eq!((at(low - 1), at(high + 1)), (default, default), "{key}");
            assert_eq!(
                (at(0), at(-1), at(i64::MAX), at(i64::MIN)),
                (default, default, default, default)
            );
        }
        // Another type, or another word, is the default; the key beside it is not.
        for odd in [
            "commands = \"loud\"\n[terminal]\nsize = \"16\"\nscrollback = 2000.5\n\
             copy-on-select = \"true\"\nkeep-days = true\nannounce = 1\nshortcuts = \"no\"\n",
            "commands = true\nterminal = 3\n",
            "commands = \"Shown\"\nterminal = [1, 2]\n",
            "commands = [\"shown\"]\n[terminal]\nsize = [16]\n",
            "commands = \"\"\nterminal = \"size = 16\"\n",
        ] {
            let found = read(odd);
            assert_eq!(found.commands, "hidden", "{odd}");
            assert_eq!(found.terminal, Terminal::default(), "{odd}");
        }
        let found = read("[terminal]\nsize = 99\nannounce = true\nshortcuts = 5\nkeep-days = 2\n");
        let want = Terminal { announce: true, keep_days: 2, ..Terminal::default() };
        assert_eq!(found.terminal, want);
    }

    #[test]
    fn the_commands_and_the_terminals_limits_are_checked_at_and_past_each_end() {
        let one = |change: TerminalChange| terminal(change).check(zones);
        for (low, high, make) in [
            (
                12,
                20,
                (|n| TerminalChange { size: Some(n), ..TerminalChange::default() })
                    as fn(i64) -> TerminalChange,
            ),
            (1_000, 50_000, |n| TerminalChange {
                scrollback: Some(n),
                ..TerminalChange::default()
            }),
            (1, 30, |n| TerminalChange { keep_days: Some(n), ..TerminalChange::default() }),
        ] {
            assert!(one(make(low)).is_ok(), "{low}");
            assert!(one(make(high)).is_ok(), "{high}");
            assert!(one(make((low + high) / 2)).is_ok());
            for past in [low - 1, high + 1, 0, -1, i64::MIN, i64::MAX] {
                let failure = one(make(past)).unwrap_err();
                assert_eq!(failure.kind, Kind::BadValue, "{past}");
                assert!(
                    !failure.detail.contains(&past.to_string()) || past == 0 || past == 1,
                    "{}",
                    failure.detail
                );
            }
        }
        // Each is named by its key under `terminal`.
        let field = |change: TerminalChange| one(change).unwrap_err().field;
        assert_eq!(
            field(TerminalChange { size: Some(11), ..TerminalChange::default() }),
            Some("terminal.size")
        );
        assert_eq!(
            field(TerminalChange { scrollback: Some(999), ..TerminalChange::default() }),
            Some("terminal.scrollback")
        );
        assert_eq!(
            field(TerminalChange { keep_days: Some(31), ..TerminalChange::default() }),
            Some("terminal.keep-days")
        );
        // The switches take either, and the words of commands, either word.
        let switches = TerminalChange {
            copy_on_select: Some(true),
            announce: Some(false),
            shortcuts: Some(true),
            ..TerminalChange::default()
        };
        assert!(one(switches).is_ok());
        for word in COMMANDS {
            assert!(
                Change { commands: Some(word.into()), ..Change::default() }.check(zones).is_ok()
            );
        }
        for odd in ["Shown", "", "show", "true", "hunter2"] {
            let change = Change { commands: Some(odd.into()), ..Change::default() };
            let failure = change.check(zones).unwrap_err();
            assert_eq!((failure.kind, failure.field), (Kind::BadValue, Some("commands")), "{odd}");
            assert!(!failure.detail.contains("hunter2"));
        }
        // The first that cannot work is the one told: the older keys first.
        let two = Change {
            commands: Some("x".into()),
            terminal: Some(TerminalChange { size: Some(99), ..TerminalChange::default() }),
            ..Change::default()
        };
        assert_eq!(two.check(zones).unwrap_err().field, Some("commands"));
        let two = Change { motion: Some("x".into()), ..two };
        assert_eq!(two.check(zones).unwrap_err().field, Some("motion"));
    }

    #[test]
    fn a_change_of_the_commands_and_the_terminal_reads_from_json_and_goes_to_the_worker_as_given() {
        let read = |value: Value| serde_json::from_value::<Change>(value);
        let all = read(json!({
            "commands": "shown",
            "terminal": {
                "size": 16, "scrollback": 5000, "copy-on-select": true,
                "keep-days": 3, "announce": true, "shortcuts": false,
            },
        }))
        .unwrap();
        assert_eq!(all.commands.as_deref(), Some("shown"));
        assert_eq!(
            all.terminal,
            Some(TerminalChange {
                size: Some(16),
                scrollback: Some(5_000),
                copy_on_select: Some(true),
                keep_days: Some(3),
                announce: Some(true),
                shortcuts: Some(false),
            })
        );
        // Any of the terminal's keys, and an empty table, which changes nothing.
        assert_eq!(
            read(json!({ "terminal": { "size": 12 } })).unwrap().terminal.unwrap().size,
            Some(12)
        );
        assert!(read(json!({ "terminal": {} })).unwrap().is_empty());
        assert!(read(json!({ "terminal": null })).unwrap().is_empty());
        assert!(!read(json!({ "terminal": { "announce": false } })).unwrap().is_empty());
        for odd in [
            json!({ "terminal": { "colour": 1 } }),
            json!({ "terminal": { "copy_on_select": true } }),
            json!({ "terminal": { "keep_days": 3 } }),
            json!({ "terminal": { "size": "14" } }),
            json!({ "terminal": { "size": 14.5 } }),
            json!({ "terminal": { "announce": "yes" } }),
            json!({ "terminal": { "shortcuts": 1 } }),
            json!({ "terminal": { "copy-on-select": null, "extra": 1 } }),
            json!({ "terminal": 14 }),
            json!({ "terminal": "size" }),
            json!({ "commands": 1 }),
            json!({ "commands": true }),
        ] {
            assert!(read(odd.clone()).is_err(), "{odd}");
        }
        // What goes to the worker is only what was given, under the names
        // the page used.
        let some = Change {
            commands: Some("shown".into()),
            terminal: Some(TerminalChange {
                keep_days: Some(9),
                copy_on_select: Some(false),
                ..TerminalChange::default()
            }),
            ..Change::default()
        };
        assert_eq!(
            json!(some),
            json!({ "commands": "shown", "terminal": { "copy-on-select": false, "keep-days": 9 } })
        );
    }

    #[test]
    fn a_change_of_the_terminals_keys_makes_the_table_after_the_other_keys_and_keeps_the_comments()
    {
        let home = Home::new("terminal-write");
        // No file: the keys are made, with the table.
        let change = Change {
            commands: Some("shown".into()),
            terminal: Some(TerminalChange {
                size: Some(16),
                copy_on_select: Some(true),
                ..TerminalChange::default()
            }),
            ..Change::default()
        };
        let found = after(&home.0, &change, zones).unwrap();
        assert_eq!(
            home.text(),
            "commands = \"shown\"\n\n[terminal]\nsize = 16\ncopy-on-select = true\n"
        );
        assert_eq!(found.commands, "shown");
        assert_eq!(
            found.terminal,
            Terminal { size: 16, copy_on_select: true, ..Terminal::default() }
        );
        // A file by hand: the comments stay, and a key that is there is changed in place.
        home.write(
            "# Mine.\ntheme = \"light\"\n\n[terminal]\n# The text.\nsize = 18 # big\n\
             announce = false\n",
        );
        let change = terminal(TerminalChange {
            size: Some(20),
            announce: Some(true),
            shortcuts: Some(false),
            keep_days: Some(1),
            ..TerminalChange::default()
        });
        after(&home.0, &change, zones).unwrap();
        assert_eq!(
            home.text(),
            "# Mine.\ntheme = \"light\"\n\n[terminal]\n# The text.\nsize = 20 # big\n\
             announce = true\nkeep-days = 1\nshortcuts = false\n"
        );
        // The same again writes nothing.
        let inode = fs::metadata(home.file()).unwrap().ino();
        after(&home.0, &change, zones).unwrap();
        assert_eq!(fs::metadata(home.file()).unwrap().ino(), inode);
    }

    #[test]
    fn a_change_of_nothing_in_the_terminal_writes_nothing_and_makes_no_table() {
        let home = Home::new("terminal-empty");
        for change in [terminal(TerminalChange::default()), Change::default()] {
            assert!(change.is_empty());
            let found = after(&home.0, &change, zones).unwrap();
            assert_eq!(found, Preferences::default());
            assert!(!home.0.join(".config").exists());
        }
        home.write("theme = \"light\"\n");
        after(&home.0, &terminal(TerminalChange::default()), zones).unwrap();
        assert_eq!(home.text(), "theme = \"light\"\n");
        // A change of the commands alone makes no table either.
        let commands = Change { commands: Some("shown".into()), ..Change::default() };
        assert!(!commands.is_empty());
        after(&home.0, &commands, zones).unwrap();
        assert_eq!(home.text(), "theme = \"light\"\ncommands = \"shown\"\n");
    }

    #[test]
    fn a_terminal_that_is_no_table_is_made_over_by_a_change_and_an_inline_table_is_kept_inline() {
        let home = Home::new("terminal-over");
        home.write("terminal = 3\ntheme = \"light\"\n");
        let change = terminal(TerminalChange { size: Some(13), ..TerminalChange::default() });
        let found = after(&home.0, &change, zones).unwrap();
        assert_eq!(found.terminal.size, 13);
        assert_eq!(home.text(), "theme = \"light\"\n\n[terminal]\nsize = 13\n");
        home.write("terminal = { size = 12 }\n");
        let change = terminal(TerminalChange { announce: Some(true), ..TerminalChange::default() });
        let found = after(&home.0, &change, zones).unwrap();
        assert!(home.text().starts_with("terminal = { size = 12"), "{}", home.text());
        assert!(!home.text().contains('['), "{}", home.text());
        assert_eq!((found.terminal.size, found.terminal.announce), (12, true));
        assert_eq!(read(&home.0).unwrap().terminal, found.terminal);
    }

    #[test]
    fn the_worker_checks_the_terminals_limits_again_and_writes_nothing_that_cannot_work() {
        let home = Home::new("terminal-again");
        for (change, field) in [
            (TerminalChange { size: Some(21), ..TerminalChange::default() }, "terminal.size"),
            (
                TerminalChange { scrollback: Some(999), ..TerminalChange::default() },
                "terminal.scrollback",
            ),
            (
                TerminalChange { keep_days: Some(0), ..TerminalChange::default() },
                "terminal.keep-days",
            ),
        ] {
            let fault = change_with(&home.0, &terminal(change), zones).unwrap_err();
            assert_eq!(fault.id, "ADM-125", "{field}");
        }
        assert!(!home.0.join(".config").exists());
        // A change that is right with one that is not writes neither.
        let both = Change {
            commands: Some("shown".into()),
            terminal: Some(TerminalChange { size: Some(30), ..TerminalChange::default() }),
            ..Change::default()
        };
        assert_eq!(change_with(&home.0, &both, zones).unwrap_err().id, "ADM-125");
        assert!(!home.0.join(".config").exists());
    }

    #[test]
    fn the_home_is_a_path_from_the_root_that_authd_gave() {
        assert_eq!(home_of(Some("/home/chase".into())), Ok(PathBuf::from("/home/chase")));
        assert!(home_of(None).unwrap_err().contains("no home folder"));
        for odd in ["", "chase", "./chase", "~"] {
            assert!(home_of(Some(odd.into())).unwrap_err().contains("not a path"), "{odd}");
        }
    }
}
