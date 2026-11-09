// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! audio.toml(5): `/system/state/audio/audio.toml`, the output to play
//! through, its volume and whether it is muted, which audiod(8) applies
//! and audio(1) and the volume keys change. It is state, as ALSA's saved
//! mixer is on Linux, rather than one of the settings of
//! `/system/settings`: no service may change those but all of them, so
//! audiod, which saves what the volume keys do, could not keep it there.
//!
//! The file is a few `key = value` lines in TOML's syntax: a string in
//! double quotes, a whole number, or true or false. Comments and blank
//! lines are kept as they are when a value is changed, and unknown keys
//! are left alone, so that a file written by hand keeps its notes.

/// Where the settings are.
pub const PATH: &str = "/system/state/audio/audio.toml";

/// What a new file starts with, before its values.
pub const HEADER: &str = "# Sound (audio.toml(5)): the output to play through, as `audio list`\n\
                          # names it (empty for the one plugged in last), the volume, 0 to\n\
                          # 100, and whether it is muted. audio(1) and the volume keys change\n\
                          # it; audiod(8) gives it to the outputs.\n";

/// The volume when the settings name none.
pub const DEFAULT_VOLUME: u8 = 70;

/// The settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The output to play through, by its name in `/devices/audio`; none
    /// for audiod's choice (the newest USB output, else the first).
    pub output: Option<String>,
    /// The volume, 0 to 100, on both channels.
    pub volume: u8,
    /// Whether the sound is muted, which keeps the volume for after.
    pub muted: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { output: None, volume: DEFAULT_VOLUME, muted: false }
    }
}

/// A line's key and value, if it is a `key = value` line. reader.toml(5)
/// is read the same way.
pub fn entry(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.starts_with('#') || line.is_empty() {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    let value = value.trim();
    // A comment after a value that is not a string.
    let value = if value.starts_with('"') {
        value
    } else {
        value.split_once('#').map_or(value, |(value, _)| value).trim()
    };
    Some((key.trim(), value))
}

/// A quoted string's contents, with `\"` and `\\` undone.
pub fn string(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
    None
}

impl Settings {
    /// Reads the settings from `text`: values that are missing or cannot
    /// be read are the defaults.
    pub fn parse(text: &str) -> Settings {
        let mut settings = Settings::default();
        for (key, value) in text.lines().filter_map(entry) {
            match key {
                "output" => settings.output = string(value).filter(|name| !name.is_empty()),
                "volume" => {
                    if let Ok(volume) = value.parse::<u32>() {
                        settings.volume = volume.min(100) as u8;
                    }
                }
                "muted" => settings.muted = value == "true",
                _ => {}
            }
        }
        settings
    }

    /// `text` with these settings in it: each key's line changed where it
    /// is, and those it lacks added at the end.
    pub fn update(&self, text: &str) -> String {
        let output = match &self.output {
            Some(name) => format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\"")),
            None => "\"\"".to_string(),
        };
        let values = [
            ("output", output),
            ("volume", self.volume.to_string()),
            ("muted", self.muted.to_string()),
        ];
        let mut seen = [false; 3];
        let mut out = String::new();
        for line in text.lines() {
            let replaced = entry(line).and_then(|(key, _)| {
                let i = values.iter().position(|(name, _)| *name == key)?;
                seen[i] = true;
                Some(format!("{key} = {}", values[i].1))
            });
            out.push_str(replaced.as_deref().unwrap_or(line));
            out.push('\n');
        }
        for ((key, value), seen) in values.iter().zip(seen) {
            if !seen {
                out.push_str(&format!("{key} = {value}\n"));
            }
        }
        out
    }

    /// The volume the outputs are given: none while muted.
    pub fn effective_volume(&self) -> u8 {
        if self.muted { 0 } else { self.volume }
    }
}
