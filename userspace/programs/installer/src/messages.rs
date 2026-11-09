// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Every message the installer shows, in Fluent (projectfluent.org): one
//! file a language, `messages/<language>.ftl`, which a translator edits
//! without touching the code (ADR 0310). The code names a message by its
//! id, `text("welcome-intro")`, and gives its variables by name.
//!
//! Only `en-US` is there until the screens are translated, so the
//! messages are built into the program. A message the file lacks shows as
//! its id in brackets, which the screens' tests look for.

use std::sync::OnceLock;

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

/// The language the installer speaks, as a BCP 47 tag.
pub const LANGUAGE: &str = "en-US";

/// The messages in [`LANGUAGE`].
const ENGLISH: &str = include_str!("../messages/en-US.ftl");

/// The messages, made the first time one is asked for.
static BUNDLE: OnceLock<FluentBundle<FluentResource>> = OnceLock::new();

/// Builds the bundle of [`ENGLISH`]. The file is part of the program and
/// its tests parse it, so a mistake in it is a bug, not a user's problem:
/// a message that does not parse is left out and shows as its id.
fn bundle() -> &'static FluentBundle<FluentResource> {
    BUNDLE.get_or_init(|| {
        let language: LanguageIdentifier = LANGUAGE.parse().expect("en-US is a language tag");
        let mut bundle = FluentBundle::new_concurrent(vec![language]);
        // Unicode's isolation marks around each variable keep a name
        // written right to left from turning the sentence around; the
        // console has no font for them, and English needs none.
        bundle.set_use_isolating(false);
        let resource = match FluentResource::try_new(ENGLISH.to_string()) {
            Ok(resource) | Err((resource, _)) => resource,
        };
        // A message defined twice keeps its first meaning.
        let _ = bundle.add_resource(resource);
        bundle
    })
}

/// The message `id`, with no variables.
#[must_use]
pub fn text(id: &str) -> String {
    with(id, &[])
}

/// The message `id`, with each variable of `args` by its name.
#[must_use]
pub fn with(id: &str, args: &[(&str, Arg<'_>)]) -> String {
    let bundle = bundle();
    let Some(pattern) = bundle.get_message(id).and_then(|message| message.value()) else {
        return missing(id);
    };
    let mut fluent_args = FluentArgs::new();
    for (name, arg) in args {
        let value = match arg {
            Arg::Text(text) => FluentValue::from(*text),
            Arg::Number(number) => FluentValue::from(*number),
        };
        fluent_args.set(*name, value);
    }
    let mut errors = Vec::new();
    let formatted = bundle.format_pattern(pattern, Some(&fluent_args), &mut errors);
    if errors.is_empty() { formatted.into_owned() } else { missing(id) }
}

/// What shows for a message the file lacks, or one whose variables the
/// code did not give.
fn missing(id: &str) -> String {
    format!("[{id}]")
}

/// A message's variable.
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    /// Text, shown as it is.
    Text(&'a str),
    /// A number, which picks a message's plural form.
    Number(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_english_file_parses_whole() {
        if let Err((_, errors)) = FluentResource::try_new(ENGLISH.to_string()) {
            panic!("messages/en-US.ftl: {errors:?}");
        }
    }

    #[test]
    fn messages_fill_in_their_variables() {
        assert_eq!(with("step-partition", &[("disk", Arg::Text("vda"))]), "Partitioning vda");
        assert_eq!(with("disk-partitions", &[("count", Arg::Number(1))]), "1 partition");
        assert_eq!(with("disk-partitions", &[("count", Arg::Number(4))]), "4 partitions");
        for (medium, said) in [("stick", "stick"), ("disc", "disc")] {
            let line = with(
                "installed-line",
                &[
                    ("version", Arg::Text("0.14.0")),
                    ("disk", Arg::Text("vda")),
                    ("medium", Arg::Text(medium)),
                ],
            );
            assert_eq!(
                line,
                format!(
                    "Dukovo 0.14.0 is installed on vda. Remove the install {said}, then restart."
                )
            );
        }
    }

    #[test]
    fn from_a_disc_no_message_names_the_stick() {
        // Shown only from a stick: its line in the disks, and its setup
        // files; and the Done page's help, which names both.
        const STICK_ONLY: [&str; 7] = [
            "disk-stick",
            "button-save-setup",
            "setup-on-stick",
            "setup-no-esp",
            "setup-saved",
            "setup-saved-no-hashes",
            "help-done",
        ];
        let bundle = bundle();
        let mut args = FluentArgs::new();
        args.set("medium", "disc");
        let ids = ENGLISH.lines().filter_map(|line| line.split_once(" =")).map(|(id, _)| id);
        let mut checked = 0;
        for id in ids.filter(|id| !id.starts_with([' ', '#', '-']) && !STICK_ONLY.contains(id)) {
            let Some(pattern) = bundle.get_message(id).and_then(|message| message.value()) else {
                continue;
            };
            // Other variables a message has show as their names.
            let said = bundle.format_pattern(pattern, Some(&args), &mut Vec::new());
            // The word, not "sticky keys".
            let stick = said.split(|c: char| !c.is_alphanumeric()).any(|word| word == "stick");
            assert!(!stick, "{id}: {said}");
            checked += 1;
        }
        assert!(checked > 100, "{checked} messages");
        // A choice inside a help page keeps the page's lines.
        let help = with("help-setup-file", &[("medium", Arg::Text("disc"))]);
        let lines: Vec<&str> = help.lines().collect();
        assert_eq!(lines.len(), 3, "{help}");
        assert_eq!(
            lines[0],
            "A setup file is an answers file (install-answers(5)) called install.json, downloaded from a web server over HTTPS only, so that it cannot be changed on the way."
        );
        assert!(lines[1].starts_with("Give its SHA-256"), "{help}");
        let help = with("help-disk", &[("medium", Arg::Text("stick"))]);
        assert!(
            help.ends_with("Other disks stay as they are. The install stick cannot be chosen.")
        );
        // A stick's own wording is still there for a stick.
        assert_eq!(
            with("setup-none", &[("medium", Arg::Text("stick"))]),
            "No setup file was found on this stick."
        );
    }

    #[test]
    fn a_missing_message_or_variable_shows_its_id() {
        assert_eq!(text("no-such-message"), "[no-such-message]");
        assert_eq!(text("step-partition"), "[step-partition]");
    }
}
