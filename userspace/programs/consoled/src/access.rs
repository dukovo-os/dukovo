// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! accessibility.toml(5), _/system/settings/console/accessibility.toml_:
//! the consoles' text size, sticky keys and slow keys (ADR 0325).
//! consoled reads it as it starts and on SIGHUP (`service consoled
//! reload`), and writes back what a person changes from the keyboard: the
//! text size with Ctrl+Alt+Plus and Ctrl+Alt+Minus, sticky keys with
//! Shift pressed five times. The installer writes it for the system it
//! installs.
//!
//! ```toml
//! text-size = "larger"    # "normal", "larger" or "largest"
//! sticky-keys = true      # Shift, Ctrl and Alt hold until the next key
//! slow-keys = false       # a key counts once held for slow-keys-delay
//! slow-keys-delay = 300   # milliseconds, 100 to 2000
//! ```

use std::io;
use std::time::Duration;

/// Where the settings are.
pub const SETTINGS: &str = "/system/settings/console/accessibility.toml";

/// What a file that is not there yet starts with when consoled writes a
/// key to it.
const HEADER: &str = "\
# The consoles' text size and keys (accessibility.toml(5)). consoled reads
# it as it starts and on `service consoled reload`, and writes the text
# size and sticky keys as they are changed from the keyboard.
";

/// The fewest and most milliseconds slow keys wait.
const DELAY_MS: (u64, u64) = (100, 2000);

/// How big the consoles' text is: the font the screen's size chooses, or
/// one or two steps bigger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    #[default]
    Normal,
    Larger,
    Largest,
}

impl Size {
    /// How many steps above the font the screen's size chooses.
    pub fn steps(self) -> usize {
        match self {
            Size::Normal => 0,
            Size::Larger => 1,
            Size::Largest => 2,
        }
    }

    /// The size `steps` above normal, at most the largest.
    pub fn from_steps(steps: usize) -> Size {
        match steps {
            0 => Size::Normal,
            1 => Size::Larger,
            _ => Size::Largest,
        }
    }

    /// Its name in accessibility.toml(5).
    pub fn name(self) -> &'static str {
        match self {
            Size::Normal => "normal",
            Size::Larger => "larger",
            Size::Largest => "largest",
        }
    }

    fn parse(name: &str) -> Option<Size> {
        [Size::Normal, Size::Larger, Size::Largest].into_iter().find(|size| size.name() == name)
    }
}

/// The settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    pub size: Size,
    /// Shift, Ctrl and Alt, pressed and let go alone, hold until the next
    /// key.
    pub sticky: bool,
    /// How long a key must be held before it counts, if slow keys are on.
    pub slow: Option<Duration>,
}

impl Default for Access {
    fn default() -> Access {
        Access { size: Size::Normal, sticky: false, slow: None }
    }
}

impl Access {
    /// Reads accessibility.toml(5)'s text: the defaults for what it leaves
    /// out, and what was wrong, each mistake leaving its key at the
    /// default.
    pub fn parse(text: &str) -> (Access, Vec<String>) {
        let mut access = Access::default();
        let mut problems = Vec::new();
        let table: toml::Table = match text.parse() {
            Ok(table) => table,
            Err(err) => return (access, vec![err.to_string().trim().replace('\n', " ")]),
        };
        let mut slow = false;
        let mut delay = Duration::from_millis(300);
        for (key, value) in &table {
            match (key.as_str(), value) {
                ("text-size", toml::Value::String(name)) => match Size::parse(name) {
                    Some(size) => access.size = size,
                    None => problems.push(format!(
                        "text-size: '{name}' is not \"normal\", \"larger\" or \"largest\""
                    )),
                },
                ("sticky-keys", toml::Value::Boolean(on)) => access.sticky = *on,
                ("slow-keys", toml::Value::Boolean(on)) => slow = *on,
                ("slow-keys-delay", toml::Value::Integer(ms))
                    if (DELAY_MS.0..=DELAY_MS.1).contains(&ms.unsigned_abs()) && *ms > 0 =>
                {
                    delay = Duration::from_millis(ms.unsigned_abs());
                }
                ("slow-keys-delay", _) => problems.push(format!(
                    "slow-keys-delay: not a number of milliseconds from {} to {}",
                    DELAY_MS.0, DELAY_MS.1
                )),
                ("text-size" | "sticky-keys" | "slow-keys", _) => {
                    problems.push(format!("{key}: not a value it takes (accessibility.toml(5))"));
                }
                (other, _) => problems.push(format!("{other}: no such key")),
            }
        }
        access.slow = slow.then_some(delay);
        (access, problems)
    }

    /// The settings in [`SETTINGS`], its mistakes logged; the defaults if
    /// it is not there.
    pub fn read() -> Access {
        let Ok(text) = std::fs::read_to_string(SETTINGS) else {
            return Access::default();
        };
        let (access, problems) = Access::parse(&text);
        for problem in problems {
            eprintln!("consoled: {SETTINGS}: {problem}");
        }
        access
    }
}

/// `text` with `key` set to `value`, a TOML value as it is written: the
/// line that set the key changed, or a line added at the end. The rest,
/// comments too, is kept.
pub fn with_key(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
    let sets_key = |l: &str| {
        let l = l.trim_start();
        l.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='))
    };
    let mut out = String::with_capacity(text.len() + line.len() + 1);
    let mut done = false;
    for l in text.lines() {
        if !done && sets_key(l) {
            out.push_str(&line);
            done = true;
        } else {
            out.push_str(l);
        }
        out.push('\n');
    }
    if !done {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Sets `key` to `value` in [`SETTINGS`], writing a new file whole and
/// then putting it in place, so that nobody reads half of one.
pub fn save(key: &str, value: &str) -> io::Result<()> {
    let text = std::fs::read_to_string(SETTINGS).unwrap_or_else(|_| HEADER.to_string());
    let new = format!("{SETTINGS}.new");
    std::fs::write(&new, with_key(&text, key, value))?;
    std::fs::rename(&new, SETTINGS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_key_and_says_what_is_wrong() {
        let (access, problems) = Access::parse(
            "text-size = \"larger\"\nsticky-keys = true\nslow-keys = true\nslow-keys-delay = 500\n",
        );
        assert_eq!(
            access,
            Access { size: Size::Larger, sticky: true, slow: Some(Duration::from_millis(500)) }
        );
        assert!(problems.is_empty());
        // Slow keys off: no delay, whatever it says.
        let (access, _) = Access::parse("slow-keys-delay = 500");
        assert_eq!(access.slow, None);
        let (access, problems) =
            Access::parse("text-size = \"huge\"\nslow-keys = true\nslow-keys-delay = 5\nfont = 1");
        assert_eq!(access.size, Size::Normal);
        assert_eq!(access.slow, Some(Duration::from_millis(300)));
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert_eq!(Access::parse("text-size = ").0, Access::default());
    }

    #[test]
    fn a_key_changed_keeps_the_rest_of_the_file() {
        let text = "# Mine.\ntext-size = \"normal\"\nsticky-keys = false\n";
        assert_eq!(
            with_key(text, "text-size", "\"larger\""),
            "# Mine.\ntext-size = \"larger\"\nsticky-keys = false\n"
        );
        assert_eq!(
            with_key(text, "slow-keys", "true"),
            "# Mine.\ntext-size = \"normal\"\nsticky-keys = false\nslow-keys = true\n"
        );
        // slow-keys does not set slow-keys-delay.
        assert_eq!(
            with_key("slow-keys-delay = 400\n", "slow-keys", "true"),
            "slow-keys-delay = 400\nslow-keys = true\n"
        );
        assert!(Access::parse(&with_key(HEADER, "sticky-keys", "true")).0.sticky);
    }
}
