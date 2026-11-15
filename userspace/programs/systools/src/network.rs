// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! network(8): the network cards and the settings netd gives them (ADR
//! 0518).
//!
//! `network show` says what each card has now and what network(5) says of
//! it, as text or as JSON, which the web console's Networking page reads.
//! `network set` changes a card's settings, at once or tried with an undo,
//! and `network keep` and `network undo` end the trial. The command line is
//! a list of words and the options `--json` and `--try SECONDS`, which may
//! come before the words or after them: [`parse`] reads it, and [`main`]
//! gives the words to the command they name.
//!
//! When it misbehaves, run `ip address` and `netd leases` for what the
//! kernel and netd hold, and read `/system/settings/network` for what
//! netd was told.

pub mod set;
mod show;

use crate::args::{Arg, Args};
use crate::trial;
use crate::util::{Fail, usage};

const USAGE: &str = "Usage: network [show] [CARD] [--json]\n       \
                     network set CARD dhcp [metered auto|yes|no] [--try SECONDS]\n       \
                     network set CARD fixed ADDRESS/PREFIX... [router ADDRESS] [dns ADDRESS...] \
                     [metered auto|yes|no] [--try SECONDS]\n       \
                     network keep | undo\n  \
                     CARD           a card's name, or its MAC address\n  \
                     --json         the cards as JSON\n  \
                     --try SECONDS  try the change: Spark undoes it after SECONDS, 10 to 3600, \
                     unless `network keep` keeps it";

/// What the command line says: its words, and the options.
#[derive(Debug, PartialEq, Eq)]
struct Command {
    /// The operands, in order: the command's name and what it takes.
    words: Vec<String>,
    json: bool,
    /// How long a change is tried, in seconds.
    seconds: Option<u32>,
}

/// Reads the command line, which is `Ok(None)` for `--help`.
fn parse(args: &mut Args) -> Result<Option<Command>, Fail> {
    let mut command = Command { words: Vec::new(), json: false, seconds: None };
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => command.json = true,
            Arg::Long(name) if name == "try" && command.seconds.is_none() => {
                command.seconds = Some(trial::seconds(&args.text()?)?);
            }
            Arg::Long(name) if name == "help" => return Ok(None),
            Arg::Operand(word) => command.words.push(word.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(Some(command))
}

/// network(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let Some(command) = parse(args)? else {
        println!("{USAGE}");
        return Ok(0);
    };
    let words: Vec<&str> = command.words.iter().map(String::as_str).collect();
    let changes = matches!(words.first(), Some(&("set" | "keep" | "undo")));
    if command.json && changes {
        return Err(usage(format!("{USAGE}\n--json goes with show")));
    }
    if command.seconds.is_some() && words.first() != Some(&"set") {
        return Err(usage(format!("{USAGE}\n--try goes with set")));
    }
    // A word that names no command is a card's name, so each command goes
    // above the arm of the card.
    match words.as_slice() {
        ["set", rest @ ..] => set::run(rest, command.seconds),
        [verb @ ("keep" | "undo")] => set::end(verb),
        [] | ["show"] => show::run(None, command.json),
        ["show", card] | [card] => show::run(Some(card), command.json),
        _ => Err(usage(format!("{USAGE}\nno such command: {}", words.join(" ")))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(items: &[&str]) -> Result<Option<Command>, Fail> {
        parse(&mut Args::new("network", items.iter().map(Into::into).collect()))
    }

    fn command(words: &[&str], json: bool) -> Option<Command> {
        Some(Command {
            words: words.iter().map(ToString::to_string).collect(),
            json,
            seconds: None,
        })
    }

    #[test]
    fn the_words_and_the_option_are_read_wherever_the_option_comes() {
        assert_eq!(read(&[]).unwrap(), command(&[], false));
        assert_eq!(read(&["show", "net0"]).unwrap(), command(&["show", "net0"], false));
        assert_eq!(read(&["--json", "show"]).unwrap(), command(&["show"], true));
        assert_eq!(read(&["show", "net0", "--json"]).unwrap(), command(&["show", "net0"], true));
        assert_eq!(read(&["net0", "--json"]).unwrap(), command(&["net0"], true));
        assert_eq!(read(&["--help"]).unwrap(), None);
        let tried = read(&["set", "net0", "dhcp", "--try", "30"]).unwrap().unwrap();
        assert_eq!((tried.words.len(), tried.seconds), (3, Some(30)));
        let first = read(&["--try=600", "set", "net0", "dhcp"]).unwrap().unwrap();
        assert_eq!(first.seconds, Some(600));
    }

    #[test]
    fn a_trial_is_ten_seconds_to_an_hour_once_and_for_set_alone() {
        for seconds in ["9", "3601", "0", "a minute", "-5"] {
            let error = read(&["set", "net0", "dhcp", "--try", seconds]).unwrap_err();
            assert!(error.usage && error.message.contains("10 to 3600"), "{seconds}");
        }
        assert!(read(&["--try", "30", "--try", "40"]).unwrap_err().usage);
        for words in [
            &["keep", "--try", "30"][..],
            &["show", "--try", "30"],
            &["set", "net0", "dhcp", "--json"],
        ] {
            let mut args = Args::new("network", words.iter().map(Into::into).collect());
            let error = main(&mut args).unwrap_err();
            assert!(error.usage, "{words:?}");
        }
    }

    #[test]
    fn an_option_it_does_not_have_is_a_mistake_in_the_command_line() {
        for items in [&["--tries", "10"][..], &["-x"], &["--json=yes"]] {
            let error = read(items).unwrap_err();
            assert!(error.usage, "{items:?}");
        }
    }

    #[test]
    fn a_command_it_does_not_have_is_a_mistake_in_the_command_line() {
        for words in [&["show", "net0", "net1"][..], &["net0", "net1"]] {
            let mut args = Args::new("network", words.iter().map(Into::into).collect());
            let error = main(&mut args).unwrap_err();
            assert!(error.usage && error.message.contains("no such command"), "{words:?}");
        }
    }
}
