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
//!   `Europe/Berlin`, or empty (the default) while the person has none.
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
}

impl Default for Preferences {
    fn default() -> Preferences {
        Preferences {
            theme: THEMES[0].into(),
            text: TEXT_SIZES[0],
            motion: MOTIONS[0].into(),
            zone: String::new(),
        }
    }
}

impl Preferences {
    /// What `document` says, a key that is not there or is not right
    /// reading as its default. `is_zone` says which zones are the
    /// machine's.
    fn of(document: &DocumentMut, is_zone: impl Fn(&str) -> bool) -> Preferences {
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
        *self == Change::default()
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
        Ok(())
    }

    /// Puts what it holds into `document`, where the file keeps it.
    fn apply(&self, document: &mut DocumentMut) {
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
            json!({ "theme": "dark", "text": 100, "motion": "full", "zone": "" })
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

    #[test]
    fn the_home_is_a_path_from_the_root_that_authd_gave() {
        assert_eq!(home_of(Some("/home/chase".into())), Ok(PathBuf::from("/home/chase")));
        assert!(home_of(None).unwrap_err().contains("no home folder"));
        for odd in ["", "chase", "./chase", "~"] {
            assert!(home_of(Some(odd.into())).unwrap_err().contains("not a path"), "{odd}");
        }
    }
}
