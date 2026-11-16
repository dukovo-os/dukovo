// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! ssh-server(8): SSH's settings and its service, in the words a person
//! uses for them rather than sshd(8)'s.
//!
//! `ssh-server show` says whether sshd is on and running, its port, what
//! sshd_config says of who may sign in and how, and the key clients know
//! the machine by, as a list or as JSON, which the web console's
//! Security page reads. `ssh-server set` changes the settings its form
//! has, with every other line of the file kept, and has sshd read the
//! file; `on` and `off` turn the service on and off as `service sshd
//! enable` and `disable` do. With `--try SECONDS`, `set` and `off` are
//! Spark's trial `ssh`, which `keep` and `undo` end. The command line is a
//! list of words and the options `--json` and `--try`, which may come
//! before the words or after them: [`parse`] reads it, and [`main`] gives
//! it to the part it names.
//!
//! The parts are [`show`], the report; [`edit`], the change as a function
//! of the file's text, with its cases in `cases.txt`; [`set`], the
//! command's steps; and [`trial`], the same steps as Spark's trial.
//!
//! When sshd does not do what the file says, `ssh-server show` says what
//! the file reads as, and `service status sshd` what the service is doing.

pub(crate) mod edit;
pub(crate) mod set;
mod show;
mod trial;

use crate::args::{Arg, Args};
use crate::servicefiles;
use crate::util::{self, Fail, fail, usage};
use edit::Change;

const USAGE: &str = "Usage: ssh-server [show] [--json]\n       \
                     ssh-server set [passwords yes|no] [keys yes|no]\n                      \
                     [allow-users NAME...|everyone] [allow-groups GROUP...|everyone]\n                      \
                     [--try SECONDS]\n       \
                     ssh-server on\n       \
                     ssh-server off [--try SECONDS]\n       \
                     ssh-server keep\n       \
                     ssh-server undo\n  \
                     --json         the settings as JSON\n  \
                     --try SECONDS  Spark undoes the change after SECONDS (10 to 3600)\n                 \
                     unless `ssh-server keep` keeps it";

/// sshd's service, whose file says whether SSH is on.
const SERVICE: &str = "sshd";

/// The trial that covers sshd's file, which Spark keeps by this name.
pub const TRIAL: &str = "ssh";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// `show`, with the option.
    Show { json: bool },
    /// `set` and the change, tried for the seconds given.
    Set(Change, Option<u32>),
    /// `on`.
    On,
    /// `off`, tried for the seconds given.
    Off(Option<u32>),
    /// `keep` or `undo`: what the trial `ssh` comes to.
    End(&'static str),
}

/// Reads the command line, which is `Ok(None)` for `--help`.
fn parse(args: &mut Args) -> Result<Option<Command>, Fail> {
    let mut words = Vec::new();
    let mut json = false;
    let mut seconds = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => json = true,
            Arg::Long(name) if name == "help" => return Ok(None),
            Arg::Long(name) if name == "try" && seconds.is_none() => {
                seconds = Some(crate::trial::seconds(&args.text()?)?);
            }
            Arg::Operand(word) => words.push(word.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let command = match words.as_slice() {
        [] | ["show"] => Command::Show { json },
        _ if json => return Err(usage(format!("{USAGE}\n--json goes with show"))),
        ["set", change @ ..] => {
            let change: Vec<String> = change.iter().map(ToString::to_string).collect();
            let change = set::change(&change).map_err(|why| usage(format!("{USAGE}\n{why}")))?;
            Command::Set(change, seconds)
        }
        ["on"] => Command::On,
        ["off"] => Command::Off(seconds),
        ["keep"] => Command::End("keep"),
        ["undo"] => Command::End("undo"),
        _ => return Err(usage(format!("{USAGE}\nno such command: {}", words.join(" ")))),
    };
    if seconds.is_some() && !matches!(command, Command::Set(..) | Command::Off(_)) {
        return Err(usage(format!("{USAGE}\n--try goes with set and off")));
    }
    Ok(Some(command))
}

/// Fails unless this is root, whom sshd's service file and Spark's control
/// socket are for.
fn require_root(verb: &str) -> Result<(), Fail> {
    if util::own_user() == 0 {
        return Ok(());
    }
    Err(fail(format!(
        "only root may turn SSH {verb}: run it with elevate, as `elevate ssh-server {verb}`"
    )))
}

/// ssh-server(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    match parse(args)? {
        None => {
            println!("{USAGE}");
            Ok(0)
        }
        Some(Command::Show { json }) => show::run(json),
        Some(Command::Set(change, None)) => {
            set::require_root()?;
            trial::not_tried(None)?;
            set::run(&change)
        }
        Some(Command::Set(change, Some(seconds))) => {
            set::require_root()?;
            trial::not_tried(Some(seconds))?;
            trial::try_set(&change, seconds)
        }
        Some(Command::On) => {
            require_root("on")?;
            trial::not_tried(None)?;
            servicefiles::enable(SERVICE, true)
        }
        Some(Command::Off(None)) => {
            require_root("off")?;
            trial::not_tried(None)?;
            servicefiles::enable(SERVICE, false)
        }
        Some(Command::Off(Some(seconds))) => {
            require_root("off")?;
            trial::not_tried(Some(seconds))?;
            trial::try_off(seconds)
        }
        Some(Command::End(verb)) => trial::end(verb),
    }
}

#[cfg(test)]
mod tests {
    use super::edit::Names;
    use super::*;

    fn read(items: &[&str]) -> Result<Option<Command>, Fail> {
        parse(&mut Args::new("ssh-server", items.iter().map(Into::into).collect()))
    }

    #[test]
    fn the_words_and_the_options_are_read_wherever_the_options_come() {
        assert_eq!(read(&[]).unwrap(), Some(Command::Show { json: false }));
        assert_eq!(read(&["show"]).unwrap(), Some(Command::Show { json: false }));
        assert_eq!(read(&["--json"]).unwrap(), Some(Command::Show { json: true }));
        assert_eq!(read(&["--json", "show"]).unwrap(), Some(Command::Show { json: true }));
        assert_eq!(read(&["show", "--json"]).unwrap(), Some(Command::Show { json: true }));
        assert_eq!(read(&["on"]).unwrap(), Some(Command::On));
        assert_eq!(read(&["off"]).unwrap(), Some(Command::Off(None)));
        assert_eq!(read(&["off", "--try", "120"]).unwrap(), Some(Command::Off(Some(120))));
        assert_eq!(read(&["--try", "10", "off"]).unwrap(), Some(Command::Off(Some(10))));
        assert_eq!(read(&["keep"]).unwrap(), Some(Command::End("keep")));
        assert_eq!(read(&["undo"]).unwrap(), Some(Command::End("undo")));
        assert_eq!(read(&["--help"]).unwrap(), None);
        let Some(Command::Set(change, None)) =
            read(&["set", "passwords", "yes", "keys", "no"]).unwrap()
        else {
            panic!("set");
        };
        assert_eq!((change.passwords, change.keys), (Some(true), Some(false)));
        // `--try` is an option, so a list of names ends before it.
        let Some(Command::Set(change, Some(3600))) =
            read(&["set", "allow-users", "ann", "bo", "--try", "3600"]).unwrap()
        else {
            panic!("set --try");
        };
        assert_eq!(change.allow_users, Some(Names::Only(vec!["ann".into(), "bo".into()])));
    }

    #[test]
    fn an_option_it_does_not_have_or_a_command_it_does_not_have_is_a_mistake() {
        for items in [
            &["--all"][..],
            &["-x"],
            &["--json=yes"],
            &["--try", "30"],
            &["show", "--try", "30"],
            &["on", "--try", "30"],
            &["keep", "--try", "30"],
            &["undo", "--try", "30"],
            &["off", "--try", "9"],
            &["off", "--try", "3601"],
            &["off", "--try", "x"],
            &["off", "--try"],
            &["off", "--try", "30", "--try", "60"],
            &["restart"],
            &["on", "off"],
            &["keep", "now"],
            &["show", "now"],
            &["on", "--json"],
            &["off", "--json"],
            &["keep", "--json"],
            &["set"],
            &["set", "--json", "passwords", "yes"],
            &["set", "passwords", "maybe"],
            &["set", "allow-users"],
        ] {
            assert!(read(items).unwrap_err().usage, "{items:?}");
        }
    }
}
