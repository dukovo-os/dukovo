// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the person has set of the desktop (windowd.json(5), ADR 0722).
//!
//! Two files hold it: the system's, [`SYSTEM`], and the person's own,
//! [`PERSONAL`] in their home folder, which is laid over the system's
//! key by key. Neither has to be there. What a file has wrong is said
//! on the standard error, by the file's name and the key's, and the
//! value from before it stays: a mistake in one key costs that key
//! alone.
//!
//! windowd writes one thing itself: the person's choice, from a
//! window's menu, of whether an application may draw a part of its
//! title bars (ADR 0720). It goes into the person's file, where
//! everything else stays as it was.

use std::collections::BTreeSet;
use std::fmt::Display;
use std::fs;
use std::io::{self, Write};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

pub mod screens;

use screens::ScreensSet;

/// The system's settings.
pub const SYSTEM: &str = "/system/settings/windowd.json";

/// The person's own, below their home folder.
pub const PERSONAL: &str = ".config/windowd.json";

/// How much faster or slower the pointer may be set: every motion
/// times 2 to this power.
const SPEED: RangeInclusive<f64> = -1.0..=1.0;

/// How far apart, in milliseconds, the two presses of a double click
/// may be set to be.
const DOUBLE_CLICK_MS: RangeInclusive<u32> = 100..=2000;

/// How long, in milliseconds, a key may be set to be held before it
/// repeats.
const REPEAT_DELAY_MS: RangeInclusive<u32> = 100..=2000;

/// How many times a second a held key may be set to repeat.
const REPEAT_RATE: RangeInclusive<u32> = 1..=100;

/// Which applications may draw a part of their windows' title bars
/// (ADR 0720).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TitleBars {
    /// Whether an application with no entry of its own has the full
    /// title bar whatever it asks. Otherwise it decides.
    pub all_full: bool,
    /// The applications that have the full title bar whatever they ask.
    pub full: BTreeSet<String>,
    /// The applications that decide, whatever the others do.
    pub shared: BTreeSet<String>,
}

impl TitleBars {
    /// Whether the application `app` may share its windows' title
    /// bars. An application of no name has no entry.
    pub fn lets(&self, app: &str) -> bool {
        if self.full.contains(app) {
            return false;
        }
        self.shared.contains(app) || !self.all_full
    }

    /// Gives the application `app` an entry of its own, which lets it
    /// share its windows' title bars or does not.
    pub fn set(&mut self, app: &str, lets: bool) {
        let (to, from) = if lets {
            (&mut self.shared, &mut self.full)
        } else {
            (&mut self.full, &mut self.shared)
        };
        from.remove(app);
        to.insert(app.to_owned());
    }
}

/// What the person has set.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Which applications may draw a part of their title bars.
    pub title_bars: TitleBars,
    /// How much faster or slower than as built the pointer moves:
    /// every motion of a mouse times 2 to this power.
    pub pointer_speed: f64,
    /// How far apart the two presses of a double click may be, in
    /// milliseconds.
    pub double_click_ms: u32,
    /// How long a key is held before it repeats, in milliseconds.
    pub repeat_delay_ms: u32,
    /// How many times a second it repeats then.
    pub repeat_rate: u32,
    /// What is set of the screens (ADR 0725).
    pub screens: ScreensSet,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            title_bars: TitleBars::default(),
            pointer_speed: 0.0,
            double_click_ms: 500,
            repeat_delay_ms: 250,
            repeat_rate: 30,
            screens: ScreensSet::default(),
        }
    }
}

/// One file as it is read: its name, and what was found wrong in it.
struct Reading<'a> {
    file: &'a str,
    wrong: Vec<String>,
}

impl Reading<'_> {
    fn complain(&mut self, key: &str, why: impl Display) {
        self.wrong.push(format!("{}: `{key}` {why}", self.file));
    }

    /// The keys of `value`, which is to be an object.
    fn object<'v>(&mut self, key: &str, value: &'v Value) -> Option<&'v Map<String, Value>> {
        let object = value.as_object();
        if object.is_none() {
            self.complain(key, "is not an object");
        }
        object
    }

    /// `value`, which is to be a whole number in `range`.
    fn whole(&mut self, key: &str, value: &Value, range: &RangeInclusive<u32>) -> Option<u32> {
        let whole = value.as_u64().and_then(|whole| u32::try_from(whole).ok());
        let whole = whole.filter(|whole| range.contains(whole));
        if whole.is_none() {
            let (least, most) = (range.start(), range.end());
            self.complain(
                key,
                format_args!("is {value}, not a whole number from {least} to {most}"),
            );
        }
        whole
    }

    /// `value`, which is to be a number in `range`.
    fn number(&mut self, key: &str, value: &Value, range: &RangeInclusive<f64>) -> Option<f64> {
        let number = value.as_f64().filter(|number| range.contains(number));
        if number.is_none() {
            let (least, most) = (range.start(), range.end());
            self.complain(key, format_args!("is {value}, not a number from {least} to {most}"));
        }
        number
    }

    /// `value`, which is to be a list of applications' names.
    fn names(&mut self, key: &str, value: &Value) -> Vec<String> {
        let Some(list) = value.as_array() else {
            self.complain(key, "is not a list of applications' names");
            return Vec::new();
        };
        let mut names = Vec::new();
        for entry in list {
            match entry.as_str() {
                Some(name) if !name.is_empty() => names.push(name.to_owned()),
                _ => self.complain(key, format_args!("has {entry}, which is not a name")),
            }
        }
        names
    }
}

impl Settings {
    /// Lays the settings of `text`, the JSON of the file `file`, over
    /// these. Returns what is wrong in the text, a line for each
    /// thing; a key that is wrong stays as it was.
    pub fn lay(&mut self, file: &str, text: &str) -> Vec<String> {
        let mut reading = Reading { file, wrong: Vec::new() };
        let root = match serde_json::from_str::<Value>(text) {
            Ok(root) => root,
            Err(error) => return vec![format!("{file}: not JSON: {error}")],
        };
        let Some(root) = root.as_object() else {
            return vec![format!("{file}: not a JSON object")];
        };
        for (key, value) in root {
            match key.as_str() {
                "screens" => {
                    self.screens.lay_screens(&mut reading, value);
                    continue;
                }
                "layouts" => {
                    self.screens.lay_layouts(&mut reading, value);
                    continue;
                }
                _ => {}
            }
            let name = |within: &str| format!("{key}.{within}");
            let Some(group) = reading.object(key, value) else { continue };
            for (within, value) in group {
                let key = name(within);
                match key.as_str() {
                    "title_bars.apps" => match value.as_str() {
                        Some("decide") => self.title_bars.all_full = false,
                        Some("full") => self.title_bars.all_full = true,
                        _ => reading
                            .complain(&key, format_args!("is {value}, not \"decide\" or \"full\"")),
                    },
                    // The two lists are laid below, once the group's
                    // other keys are.
                    "title_bars.full" | "title_bars.shared" => {}
                    "pointer.speed" => {
                        let speed = reading.number(&key, value, &SPEED);
                        self.pointer_speed = speed.unwrap_or(self.pointer_speed);
                    }
                    "pointer.double_click_ms" => {
                        let ms = reading.whole(&key, value, &DOUBLE_CLICK_MS);
                        self.double_click_ms = ms.unwrap_or(self.double_click_ms);
                    }
                    "keyboard.repeat_delay_ms" => {
                        let ms = reading.whole(&key, value, &REPEAT_DELAY_MS);
                        self.repeat_delay_ms = ms.unwrap_or(self.repeat_delay_ms);
                    }
                    "keyboard.repeat_rate" => {
                        let rate = reading.whole(&key, value, &REPEAT_RATE);
                        self.repeat_rate = rate.unwrap_or(self.repeat_rate);
                    }
                    _ => reading.complain(&key, "is not a setting windowd knows"),
                }
            }
            if key == "title_bars" {
                // `full` last: an application in both lists of one
                // file has the full bar.
                for (list, lets) in [("shared", true), ("full", false)] {
                    let Some(value) = group.get(list) else { continue };
                    for app in reading.names(&name(list), value) {
                        self.title_bars.set(&app, lets);
                    }
                }
            }
        }
        reading.wrong
    }

    /// The settings of the files `files`, each laid over those before
    /// it, with what is wrong in them. A file that is not there is not
    /// wrong.
    pub fn read<'a>(files: impl IntoIterator<Item = &'a Path>) -> (Settings, Vec<String>) {
        let mut settings = Settings::default();
        let mut wrong = Vec::new();
        for file in files {
            let name = file.display().to_string();
            match fs::read_to_string(file) {
                Ok(text) => wrong.extend(settings.lay(&name, &text)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => wrong.push(format!("{name}: {error}")),
            }
        }
        (settings, wrong)
    }
}

/// The person's settings file, if the environment says where their
/// home folder is.
pub fn personal() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(Path::new(&home).join(PERSONAL))
}

/// The text of a settings file that had `text`, or was not there, once
/// the application `app` has an entry of its own in it that lets it
/// share its windows' title bars or does not. Everything else in the
/// file stays; its keys are written in the alphabet's order.
///
/// # Errors
///
/// Why the entry cannot be put into the text: the text is not what a
/// settings file is, and is not to be written over.
pub fn with_app(text: Option<&str>, app: &str, lets: bool) -> Result<String, String> {
    let mut root = match text {
        Some(text) => {
            serde_json::from_str::<Value>(text).map_err(|err| format!("not JSON: {err}"))?
        }
        None => Value::Object(Map::new()),
    };
    let root = root.as_object_mut().ok_or("not a JSON object")?;
    let bars = root.entry("title_bars").or_insert_with(|| Value::Object(Map::new()));
    let bars = bars.as_object_mut().ok_or("`title_bars` is not an object")?;
    for (list, has) in [("full", !lets), ("shared", lets)] {
        if !has && !bars.contains_key(list) {
            continue;
        }
        let names = bars.entry(list).or_insert_with(|| Value::Array(Vec::new()));
        let names =
            names.as_array_mut().ok_or_else(|| format!("`title_bars.{list}` is not a list"))?;
        names.retain(|name| name.as_str() != Some(app));
        if has {
            names.push(Value::String(app.to_owned()));
        }
    }
    let mut text = serde_json::to_string_pretty(root).map_err(|err| err.to_string())?;
    text.push('\n');
    Ok(text)
}

/// Gives the application `app` an entry of its own in the settings
/// file `file`, as [`with_app`] does. The file is written whole beside
/// the old one and then takes its place, so that no reader finds half
/// of it.
///
/// # Errors
///
/// Why the file was not written, for the person to read.
pub fn keep_app(file: &Path, app: &str, lets: bool) -> Result<(), String> {
    let said = |why: &dyn Display| format!("{}: {why}", file.display());
    let before = match fs::read_to_string(file) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(said(&error)),
    };
    let text = with_app(before.as_deref(), app, lets).map_err(|why| said(&why))?;
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder).map_err(|err| said(&err))?;
    }
    let mut beside = file.as_os_str().to_owned();
    beside.push(".new");
    let beside = PathBuf::from(beside);
    let written = fs::File::create(&beside).and_then(|mut new| {
        new.write_all(text.as_bytes())?;
        new.sync_all()
    });
    written.and_then(|()| fs::rename(&beside, file)).map_err(|err| said(&err))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn laid(texts: &[&str]) -> (Settings, Vec<String>) {
        let mut settings = Settings::default();
        let mut wrong = Vec::new();
        for (index, text) in texts.iter().enumerate() {
            wrong.extend(settings.lay(["system", "personal"][index], text));
        }
        (settings, wrong)
    }

    #[test]
    fn an_application_decides_unless_the_person_says_otherwise() {
        let mut bars = TitleBars::default();
        assert!(bars.lets("org.example.browser"));
        assert!(bars.lets(""));
        bars.set("org.example.browser", false);
        assert!(!bars.lets("org.example.browser"));
        assert!(bars.lets("org.example.editor"));
        bars.set("org.example.browser", true);
        assert!(bars.lets("org.example.browser"));
        assert!(bars.full.is_empty());
    }

    #[test]
    fn an_applications_own_entry_wins_over_the_rule_for_all() {
        let mut bars = TitleBars { all_full: true, ..TitleBars::default() };
        assert!(!bars.lets("org.example.browser"));
        bars.set("org.example.browser", true);
        assert!(bars.lets("org.example.browser"));
        assert!(!bars.lets("org.example.editor"));
        bars.set("org.example.browser", false);
        assert!(!bars.lets("org.example.browser"));
        assert!(bars.shared.is_empty());
    }

    #[test]
    fn with_no_file_the_settings_are_as_built() {
        let (settings, wrong) = laid(&[]);
        assert_eq!(settings, Settings::default());
        assert!(wrong.is_empty());
        assert_eq!((settings.double_click_ms, settings.repeat_delay_ms), (500, 250));
        assert_eq!((settings.repeat_rate, settings.pointer_speed), (30, 0.0));
    }

    #[test]
    fn the_persons_file_is_laid_over_the_systems_key_by_key() {
        let system = r#"{
            "title_bars": { "apps": "full", "shared": ["org.example.editor", "org.example.mail"] },
            "pointer": { "speed": 0.5, "double_click_ms": 400 },
            "keyboard": { "repeat_delay_ms": 300, "repeat_rate": 25 }
        }"#;
        let personal = r#"{
            "title_bars": { "full": ["org.example.mail"] },
            "pointer": { "speed": -1 },
            "keyboard": { "repeat_rate": 40 }
        }"#;
        let (settings, wrong) = laid(&[system, personal]);
        assert_eq!(wrong, [] as [&str; 0]);
        assert_eq!(settings.pointer_speed, -1.0);
        assert_eq!(settings.double_click_ms, 400);
        assert_eq!((settings.repeat_delay_ms, settings.repeat_rate), (300, 40));
        let bars = &settings.title_bars;
        assert!(bars.all_full);
        assert!(bars.lets("org.example.editor"));
        // The person's entry for an application wins over the system's.
        assert!(!bars.lets("org.example.mail"));
        assert!(!bars.lets("org.example.browser"));
    }

    #[test]
    fn an_application_in_both_lists_of_a_file_has_the_full_bar() {
        let (settings, wrong) =
            laid(&[r#"{ "title_bars": { "full": ["a.b"], "shared": ["a.b", "c.d"] } }"#]);
        assert!(wrong.is_empty());
        assert!(!settings.title_bars.lets("a.b"));
        assert_eq!(settings.title_bars.shared.len(), 1);
    }

    #[test]
    fn what_is_wrong_is_named_and_costs_its_own_key_alone() {
        let system = r#"{ "pointer": { "speed": 0.5 }, "keyboard": { "repeat_rate": 25 } }"#;
        let personal = r#"{
            "title_bars": { "apps": "never", "full": "org.example.mail", "shared": ["a.b", 7, ""] },
            "pointer": { "speed": 3, "double_click_ms": 50, "colour": "red" },
            "keyboard": { "repeat_delay_ms": 250.5, "repeat_rate": 0 },
            "sound": true,
            "screens": {}
        }"#;
        let (settings, wrong) = laid(&[system, personal]);
        assert_eq!(
            wrong,
            [
                "personal: `keyboard.repeat_delay_ms` is 250.5, not a whole number from 100 to 2000",
                "personal: `keyboard.repeat_rate` is 0, not a whole number from 1 to 100",
                "personal: `pointer.colour` is not a setting windowd knows",
                "personal: `pointer.double_click_ms` is 50, not a whole number from 100 to 2000",
                "personal: `pointer.speed` is 3, not a number from -1 to 1",
                "personal: `sound` is not an object",
                "personal: `title_bars.apps` is \"never\", not \"decide\" or \"full\"",
                "personal: `title_bars.shared` has 7, which is not a name",
                "personal: `title_bars.shared` has \"\", which is not a name",
                "personal: `title_bars.full` is not a list of applications' names",
            ]
        );
        // What was right in the file holds, and what was wrong is as
        // the file before had it, or as built.
        assert_eq!((settings.pointer_speed, settings.repeat_rate), (0.5, 25));
        assert_eq!((settings.double_click_ms, settings.repeat_delay_ms), (500, 250));
        assert!(settings.title_bars.shared.contains("a.b"));
        assert!(!settings.title_bars.all_full);
    }

    #[test]
    fn a_file_that_is_not_json_or_not_an_object_changes_nothing() {
        let (settings, wrong) = laid(&[r#"{ "pointer": { "speed": 1 } }"#, "{ pointer: fast }"]);
        assert_eq!(settings.pointer_speed, 1.0);
        assert_eq!(wrong.len(), 1);
        assert!(wrong[0].starts_with("personal: not JSON: "), "{wrong:?}");
        let (settings, wrong) = laid(&["[1, 2]"]);
        assert_eq!(settings, Settings::default());
        assert_eq!(wrong, ["system: not a JSON object"]);
    }

    #[test]
    fn an_applications_entry_is_written_and_everything_else_stays() {
        let before = r#"{
            "theme": "dusk",
            "title_bars": { "apps": "full", "full": ["a.b", "c.d"], "later": 1 },
            "pointer": { "speed": 0.5 }
        }"#;
        let after = with_app(Some(before), "a.b", true).unwrap();
        let read: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(
            read,
            serde_json::json!({
                "theme": "dusk",
                "title_bars": { "apps": "full", "full": ["c.d"], "shared": ["a.b"], "later": 1 },
                "pointer": { "speed": 0.5 }
            })
        );
        assert!(after.ends_with("}\n"));
        // And back: one entry for an application, never two.
        let back = with_app(Some(&after), "a.b", false).unwrap();
        let read: Value = serde_json::from_str(&back).unwrap();
        assert_eq!(read["title_bars"]["full"], serde_json::json!(["c.d", "a.b"]));
        assert_eq!(read["title_bars"]["shared"], serde_json::json!([]));
        let mut settings = Settings::default();
        assert!(settings.lay("personal", &back).len() == 2, "theme and later are not known");
        assert!(!settings.title_bars.lets("a.b"));
    }

    #[test]
    fn a_file_that_is_not_there_is_begun_and_one_that_is_not_settings_is_left() {
        let new = with_app(None, "a.b", false).unwrap();
        assert_eq!(new, "{\n  \"title_bars\": {\n    \"full\": [\n      \"a.b\"\n    ]\n  }\n}\n");
        assert_eq!(
            with_app(Some("{ nope"), "a.b", false).unwrap_err().split(':').next(),
            Some("not JSON")
        );
        assert_eq!(with_app(Some("[]"), "a.b", false).unwrap_err(), "not a JSON object");
        let wrong = with_app(Some(r#"{ "title_bars": { "full": "a.b" } }"#), "a.b", false);
        assert_eq!(wrong.unwrap_err(), "`title_bars.full` is not a list");
        let wrong = with_app(Some(r#"{ "title_bars": 3 }"#), "a.b", true);
        assert_eq!(wrong.unwrap_err(), "`title_bars` is not an object");
    }

    #[test]
    fn the_files_are_read_in_their_order_and_an_entry_is_kept_in_the_persons() {
        let folder = std::env::temp_dir().join(format!("windowd-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        let (system, personal) = (folder.join("system.json"), folder.join("home/.config/own.json"));
        fs::write(&system, r#"{ "pointer": { "speed": 1 }, "keyboard": { "repeat_rate": 200 } }"#)
            .unwrap();
        // The person's is not there yet, which is not wrong.
        let (settings, wrong) = Settings::read([system.as_path(), personal.as_path()]);
        assert_eq!(settings.pointer_speed, 1.0);
        assert_eq!(wrong.len(), 1, "{wrong:?}");
        assert!(wrong[0].contains("system.json: `keyboard.repeat_rate` is 200"), "{wrong:?}");
        // Kept, it is there, in a folder made for it, and nothing is
        // left beside it.
        keep_app(&personal, "org.example.browser", false).unwrap();
        keep_app(&personal, "org.example.editor", false).unwrap();
        keep_app(&personal, "org.example.browser", true).unwrap();
        let (settings, _) = Settings::read([system.as_path(), personal.as_path()]);
        assert!(settings.title_bars.lets("org.example.browser"));
        assert!(!settings.title_bars.lets("org.example.editor"));
        let beside: Vec<_> = fs::read_dir(personal.parent().unwrap()).unwrap().collect();
        assert_eq!(beside.len(), 1);
        // A file that is not settings is not written over.
        fs::write(&personal, "not settings").unwrap();
        let refused = keep_app(&personal, "org.example.editor", true).unwrap_err();
        assert!(refused.contains("own.json: not JSON"), "{refused}");
        assert_eq!(fs::read_to_string(&personal).unwrap(), "not settings");
        fs::remove_dir_all(&folder).unwrap();
    }
}
