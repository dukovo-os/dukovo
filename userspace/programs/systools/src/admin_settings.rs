// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console's settings, `/system/settings/admin.toml`
//! (admin.toml(5), ADR 0514): the sign-in rules, which authd(8) keeps,
//! and where admind(8) answers and whom.
//!
//! A file that is not there gives the defaults; a key that is not there
//! gives its default; a key with a value out of its range is refused, so
//! that a typo never loosens a rule unnoticed.

use std::path::Path;

use serde::Deserialize;

/// The file.
pub const PATH: &str = "/system/settings/admin.toml";

/// Who must give a second step at sign-in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecondStep {
    /// Those who have added one.
    #[default]
    IfSet,
    /// Administrators, and those who have added one.
    Administrators,
    /// Everyone.
    Everyone,
}

/// Whom admind answers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reach {
    /// Machines on private networks: this one, its own networks, and the
    /// private, shared and local address ranges.
    #[default]
    Private,
    /// Any machine.
    Any,
}

impl Reach {
    /// The word admin.toml and the console's API write it with.
    pub fn word(self) -> &'static str {
        match self {
            Reach::Private => "private",
            Reach::Any => "any",
        }
    }

    /// The reach that `word` names.
    pub fn parse(word: &str) -> Option<Reach> {
        [Reach::Private, Reach::Any].into_iter().find(|reach| reach.word() == word)
    }
}

/// The `[sign-in]` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct SignIn {
    /// Who gives a second step.
    pub second_step: SecondStep,
    /// The hours a session lasts after its sign-in, 1 to 168.
    pub session_hours: u32,
    /// The minutes a session lasts with no console page open, 5 to 1440.
    pub idle_minutes: u32,
}

impl Default for SignIn {
    fn default() -> SignIn {
        SignIn { second_step: SecondStep::IfSet, session_hours: 12, idle_minutes: 30 }
    }
}

/// The `[console]` table.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Console {
    /// The port admind answers on: 443, or 1024 to 65535.
    pub listen: u16,
    /// Whom it answers.
    pub reach: Reach,
    /// The certificate it serves, by certd's name for it.
    pub certificate: String,
}

impl Default for Console {
    fn default() -> Console {
        Console { listen: 9090, reach: Reach::Private, certificate: "this-machine".into() }
    }
}

/// The whole file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Settings {
    /// The sign-in rules.
    pub sign_in: SignIn,
    /// The console's server.
    pub console: Console,
}

impl Settings {
    /// The settings in `text`, checked.
    pub fn parse(text: &str) -> Result<Settings, String> {
        let settings: Settings =
            toml::from_str(text).map_err(|err| format!("{PATH}: {}", err.message()))?;
        let sign_in = &settings.sign_in;
        if !(1..=168).contains(&sign_in.session_hours) {
            return Err(format!("{PATH}: session-hours is 1 to 168"));
        }
        if !(5..=1440).contains(&sign_in.idle_minutes) {
            return Err(format!("{PATH}: idle-minutes is 5 to 1440"));
        }
        let port = settings.console.listen;
        if port != 443 && port < 1024 {
            return Err(format!("{PATH}: listen is 443, or 1024 to 65535"));
        }
        if let Err(err) = manifest::service::check_certificate(&settings.console.certificate) {
            return Err(format!("{PATH}: certificate: {err}"));
        }
        Ok(settings)
    }

    /// The settings in [`PATH`], or the defaults when it is not there.
    pub fn read() -> Result<Settings, String> {
        Settings::read_at(Path::new(PATH))
    }

    /// The settings in the file `path`, which a test keeps elsewhere than
    /// [`PATH`], or the defaults when it is not there.
    pub fn read_at(path: &Path) -> Result<Settings, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Settings::parse(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(err) => Err(format!("cannot read {}: {err}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_gives_the_defaults() {
        let settings = Settings::parse("").unwrap();
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.sign_in.second_step, SecondStep::IfSet);
        assert_eq!((settings.sign_in.session_hours, settings.sign_in.idle_minutes), (12, 30));
        assert_eq!(settings.console.listen, 9090);
        assert_eq!(settings.console.reach, Reach::Private);
        assert_eq!(settings.console.certificate, "this-machine");
    }

    #[test]
    fn a_reach_is_named_by_the_word_the_file_reads_it_by() {
        for reach in [Reach::Private, Reach::Any] {
            let text = format!("[console]\nreach = \"{}\"\n", reach.word());
            assert_eq!(Settings::parse(&text).unwrap().console.reach, reach);
            assert_eq!(Reach::parse(reach.word()), Some(reach));
        }
        for word in ["", "Any", "public", "private ", "everyone"] {
            assert_eq!(Reach::parse(word), None, "{word:?}");
        }
    }

    #[test]
    fn a_file_that_is_not_there_gives_the_defaults_and_one_that_is_gives_its_settings() {
        let dir = std::env::temp_dir().join(format!("admin-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("admin.toml");
        assert_eq!(Settings::read_at(&path).unwrap(), Settings::default());
        std::fs::write(&path, "[console]\nreach = \"any\"\n").unwrap();
        assert_eq!(Settings::read_at(&path).unwrap().console.reach, Reach::Any);
        let unreadable = Settings::read_at(&dir).unwrap_err();
        assert!(
            unreadable.starts_with(&format!("cannot read {}: ", dir.display())),
            "{unreadable}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_key_reads() {
        let settings = Settings::parse(
            "[sign-in]\nsecond-step = \"administrators\"\nsession-hours = 1\nidle-minutes = 1440\n\
             [console]\nlisten = 443\nreach = \"any\"\ncertificate = \"admin.example.org\"\n",
        )
        .unwrap();
        assert_eq!(settings.sign_in.second_step, SecondStep::Administrators);
        assert_eq!((settings.sign_in.session_hours, settings.sign_in.idle_minutes), (1, 1440));
        assert_eq!(settings.console.listen, 443);
        assert_eq!(settings.console.reach, Reach::Any);
        assert_eq!(settings.console.certificate, "admin.example.org");
        let everyone = Settings::parse("[sign-in]\nsecond-step = \"everyone\"\n").unwrap();
        assert_eq!(everyone.sign_in.second_step, SecondStep::Everyone);
    }

    #[test]
    fn a_value_out_of_range_or_a_typo_is_refused() {
        for text in [
            "[sign-in]\nsession-hours = 0\n",
            "[sign-in]\nsession-hours = 169\n",
            "[sign-in]\nidle-minutes = 4\n",
            "[sign-in]\nsecond-step = \"never\"\n",
            "[sign-in]\nsecond-steps = \"everyone\"\n",
            "[console]\nlisten = 80\n",
            "[console]\nlisten = 70000\n",
            "[console]\nreach = \"public\"\n",
            "[console]\ncertificate = \"../key\"\n",
            "[consoles]\n",
        ] {
            assert!(Settings::parse(text).is_err(), "{text}");
        }
    }
}
