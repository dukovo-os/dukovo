// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! database(8): PostgreSQL's databases, made, opened to other machines
//! and removed, for the web console and for people at a terminal.
//!
//! `database` signs in to PostgreSQL over its socket as the role postgres,
//! which the server lets root be by peer authentication, and speaks
//! PostgreSQL's protocol itself: [`wire`] is the protocol, and [`scram`]
//! makes a role's password and the verifier PostgreSQL keeps in its
//! place, so that the password never reaches the server.
//!
//! - [`add`] makes a database and the role that owns it, changes a
//!   role's password and removes both;
//! - [`allow`] lets other machines in over TLS and shuts them out, in
//!   `pg_hba.conf` ([`hba`]), the server's settings ([`settings`]) and
//!   the service's certificates;
//! - [`list`] says what is there, and [`copies`] the copies that backups
//!   made of a database;
//! - [`restore`] puts one of those copies back, into a new database or
//!   over the database itself, which is kept;
//! - [`names`] is what a name, a sorting and a network may be, [`sql`] is
//!   every statement sent, [`errors`] the refusals that have an ID, and
//!   [`server`] the machine around the statements, which a test stands
//!   in for.
//!
//! The password of a role is shown once, in the JSON of `add --json` and
//! `password --json`, and is in no other output, log line, error or
//! statement.

use std::ffi::OsString;
use std::io::{self, Write};

use ring::rand::SystemRandom;

use crate::args::{Arg, Args};
use crate::cert::acme::steps::Steps;
use crate::json;
use crate::util::{self, Fail, fail, usage};

mod add;
mod allow;
mod copies;
mod errors;
#[cfg(test)]
mod fake;
mod hba;
mod list;
pub(crate) mod names;
mod restore;
pub mod scram;
mod server;
mod settings;
mod sql;
pub mod wire;

use add::{Add, Remove};
use allow::{Allow, Deny};
use names::Network;
use restore::{Restore, Target};
use server::{Lock, Machine};

/// What `database` says of itself.
const HELP: &str = "Usage: database list [--json]
       database add NAME [--owner ROLE] [--peer ACCOUNT] [--locale SORTING] [--site SITE] --json
       database password NAME --json
       database allow NAME --from NETWORK... [--certificate NAME] [--progress]
       database deny NAME [--from NETWORK]... [--progress]
       database remove NAME [--role] [--force]
       database copies NAME [--json]
       database restore NAME --from FILE [--as NEW | --over [--force]] [--progress] [--json]

Makes and removes the databases of PostgreSQL, as database(8) describes. Every
command needs root; try elevate database.

  list      the server, its databases, roles and who may connect
  add       makes the database NAME and the role that owns it, which is
            ROLE, or NAME when --owner is not given; the role signs in with
            a password that is made now and shown once, in the JSON that
            --json prints, or, with --peer, as the account ACCOUNT by the
            socket, with none; SORTING is en_US.UTF-8 (the default) or
            C.UTF-8; SITE is the website it is for
  password  makes a new password for the role that owns NAME, shown once in
            the JSON that --json prints
  allow     lets the networks, such as 192.168.1.0/24, sign in to NAME from
            other machines, over TLS with the certificate NAME (this-machine
            when --certificate is not given); PostgreSQL restarts when it
            has to listen on the network or read another certificate
  deny      shuts the networks out again, or every one when --from is not
            given; PostgreSQL listens on this machine only again when none
            is left
  remove    removes the database, which must have no open session unless
            --force ends them; --role removes its role too, when database
            made it and it owns nothing else
  copies    lists the copies of NAME that backups made, the newest first: the
            one in use, and those in the snapshots of /shared
  restore   puts the copy FILE, one that `copies` lists or a pg_dump -Fc file,
            into a new database NEW, with the owner of NAME (NAME_restored_DATE
            when --as is not given), or with --over into NAME itself, which is
            kept as NAME_before_DATE_TIME; --force ends the sessions open to NAME
  --json    prints one line of JSON
  --progress  tells each step of allow, deny and restore as `step N of M: WORDS`

A change takes the lock of database, so that two do not mix, and writes
pg_hba.conf only between the two comment lines it keeps there.";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// `list`.
    List {
        /// Whether to print JSON.
        json: bool,
    },
    /// `add`.
    Add {
        /// What to make.
        ask: Add,
        /// Whether to print JSON, which a password is told in.
        json: bool,
    },
    /// `password`.
    Password {
        /// The database whose role gets it.
        name: String,
    },
    /// `allow`.
    Allow {
        /// What to allow.
        ask: Allow,
        /// Whether to tell the steps.
        progress: bool,
    },
    /// `deny`.
    Deny {
        /// What to shut out.
        ask: Deny,
        /// Whether to tell the steps.
        progress: bool,
    },
    /// `remove`.
    Remove(Remove),
    /// `copies`.
    Copies {
        /// The database.
        name: String,
        /// Whether to print JSON.
        json: bool,
    },
    /// `restore`.
    Restore {
        /// What to put back, and where.
        ask: Restore,
        /// Whether to tell the steps.
        progress: bool,
        /// Whether to print JSON.
        json: bool,
    },
}

/// What [`parse`] found.
#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    /// `--help`.
    Help,
    /// A command to run.
    Run(Command),
}

/// database(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let command = match parse(args)? {
        Parsed::Help => {
            println!("{HELP}");
            return Ok(0);
        }
        Parsed::Run(command) => command,
    };
    root_only()?;
    let mut host = Machine::new();
    run(&mut host, command)
}

/// Fails unless the program runs as root, whom PostgreSQL lets in.
fn root_only() -> Result<(), Fail> {
    if util::own_user() == 0 {
        Ok(())
    } else {
        Err(fail("only root runs database; try elevate database".into()))
    }
}

/// Runs `command` on `host`.
fn run(host: &mut Machine, command: Command) -> Result<i32, Fail> {
    match command {
        Command::List { json } => {
            let list = list::list(host)?;
            if json {
                json::print(&list)?;
            } else {
                print!("{}", list::text(&list));
            }
        }
        Command::Add { ask, json } => {
            let _lock = Lock::system()?;
            let added = add::add(host, &ask, &SystemRandom::new())?;
            show(&added, json)?;
        }
        Command::Password { name } => {
            let _lock = Lock::system()?;
            let added = add::password(host, &name, &SystemRandom::new())?;
            show(&added, true)?;
        }
        Command::Allow { ask, progress } => {
            let _lock = Lock::system()?;
            let steps = if progress { Steps::new(allow::ALLOW_STEPS) } else { Steps::off() };
            say(&allow::allow(host, &ask, &steps)?);
        }
        Command::Deny { ask, progress } => {
            let _lock = Lock::system()?;
            let steps = if progress { Steps::new(allow::DENY_STEPS) } else { Steps::off() };
            say(&allow::deny(host, &ask, &steps)?);
        }
        Command::Remove(ask) => {
            let _lock = Lock::system()?;
            say(&add::remove(host, &ask)?);
        }
        Command::Copies { name, json } => {
            let found = copies::of(host, &name);
            if json {
                json::print(&found)?;
            } else {
                print!("{}", copies::text(&found));
            }
        }
        Command::Restore { ask, progress, json } => {
            let _lock = Lock::system()?;
            let steps =
                if progress { Steps::new(restore::steps_of(&ask.target)) } else { Steps::off() };
            let restored = restore::restore(host, &ask, &steps)?;
            if json {
                json::print(&restored)?;
            } else {
                say(&restored.words());
            }
        }
    }
    Ok(0)
}

/// Prints `lines`.
fn say(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

/// Tells what `add` or `password` made: as the JSON line, which holds the
/// password and is written once and then overwritten, or, for a role with
/// no password, in words.
fn show(added: &add::Added, json: bool) -> Result<(), Fail> {
    if !json {
        println!(
            "made the database {}, owned by the role {}, which signs in as the account of that \
             name by the socket",
            added.database, added.role
        );
        return Ok(());
    }
    let mut line = added.line()?.into_bytes();
    line.push(b'\n');
    let written = io::stdout().lock().write_all(&line);
    accounts::wipe(&mut line);
    written.map_err(Fail::from)
}

/// What the command line says.
fn parse(args: &mut Args) -> Result<Parsed, Fail> {
    let Some(name) = args.subcommand() else {
        return match args.next()? {
            Some(Arg::Long(name)) if name == "help" => Ok(Parsed::Help),
            Some(other) => Err(args.unknown(&other)),
            None => Err(usage(format!("a command is needed\n{HELP}"))),
        };
    };
    let command = match name.to_str() {
        Some("list") => Command::List { json: only_json(args)? },
        Some("add") => add_options(args)?,
        Some("password") => password_options(args)?,
        Some("allow") => allow_options(args)?,
        Some("deny") => deny_options(args)?,
        Some("remove") => remove_options(args)?,
        Some("copies") => copies_options(args)?,
        Some("restore") => restore_options(args)?,
        _ => return Err(usage(format!("no such command: {}\n{HELP}", name.to_string_lossy()))),
    };
    Ok(Parsed::Run(command))
}

/// Text from an operand.
fn text_of(operand: OsString) -> Result<String, Fail> {
    operand
        .into_string()
        .map_err(|operand| usage(format!("'{}' is not UTF-8", operand.to_string_lossy())))
}

/// The one operand, the name of the database.
fn take_name(slot: &mut Option<String>, operand: OsString) -> Result<(), Fail> {
    if slot.is_some() {
        return Err(usage(format!("unexpected argument '{}'", operand.to_string_lossy())));
    }
    *slot = Some(text_of(operand)?);
    Ok(())
}

/// The name of a database from what was asked, checked by the rule.
fn checked_name(name: Option<String>) -> Result<String, Fail> {
    let name = name.ok_or_else(|| usage("the name of a database is needed".into()))?;
    names::check_name(&name).map_err(usage)?;
    Ok(name)
}

/// A network the command line names.
fn network(text: &str) -> Result<Network, Fail> {
    Network::parse(text).map_err(usage)
}

/// Whether `--json` is the only option left.
fn only_json(args: &mut Args) -> Result<bool, Fail> {
    let mut json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(json)
}

/// The options of `add`.
fn add_options(args: &mut Args) -> Result<Command, Fail> {
    let (mut name, mut owner, mut peer, mut site) = (None, None, None, None);
    let (mut locale, mut json) = (names::DEFAULT_LOCALE.to_string(), false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "owner" => owner = Some(args.text()?),
            Arg::Long(option) if option == "peer" => peer = Some(args.text()?),
            Arg::Long(option) if option == "locale" => locale = args.text()?,
            Arg::Long(option) if option == "site" => site = Some(args.text()?),
            Arg::Long(option) if option == "json" => json = true,
            Arg::Operand(operand) => take_name(&mut name, operand)?,
            other => return Err(args.unknown(&other)),
        }
    }
    let name = checked_name(name)?;
    let owner = match (&owner, &peer) {
        (Some(owner), Some(peer)) if owner != peer => {
            return Err(usage(format!(
                "--peer {peer} makes the role {peer}, which --owner {owner} is not"
            )));
        }
        (Some(owner), _) => owner.clone(),
        (None, Some(peer)) => peer.clone(),
        (None, None) => name.clone(),
    };
    names::check_name(&owner).map_err(usage)?;
    if let Some(peer) = &peer {
        names::check_name(peer).map_err(usage)?;
    }
    names::check_locale(&locale).map_err(usage)?;
    if let Some(site) = &site {
        names::check_site(site).map_err(usage)?;
    }
    if peer.is_none() && !json {
        return Err(usage(
            "add makes a password and shows it once, in JSON: run it with --json, or with --peer \
             ACCOUNT for a role that signs in as an account and has none"
                .into(),
        ));
    }
    Ok(Command::Add { ask: Add { name, owner, peer, locale, site }, json })
}

/// The options of `password`.
fn password_options(args: &mut Args) -> Result<Command, Fail> {
    let (mut name, mut json) = (None, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "json" => json = true,
            Arg::Operand(operand) => take_name(&mut name, operand)?,
            other => return Err(args.unknown(&other)),
        }
    }
    let name = checked_name(name)?;
    if !json {
        return Err(usage(
            "password shows the new password once, in JSON: run it with --json".into(),
        ));
    }
    Ok(Command::Password { name })
}

/// The options of `allow`.
fn allow_options(args: &mut Args) -> Result<Command, Fail> {
    let (mut name, mut networks, mut progress) = (None, Vec::new(), false);
    let mut certificate = String::from("this-machine");
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "from" => networks.push(network(&args.text()?)?),
            Arg::Long(option) if option == "certificate" => certificate = args.text()?,
            Arg::Long(option) if option == "progress" => progress = true,
            Arg::Operand(operand) => take_name(&mut name, operand)?,
            other => return Err(args.unknown(&other)),
        }
    }
    let name = checked_name(name)?;
    if networks.is_empty() {
        return Err(usage(
            "allow needs at least one --from NETWORK, such as 192.168.1.0/24".into(),
        ));
    }
    names::check_networks(&networks).map_err(usage)?;
    manifest::service::check_certificate(&certificate).map_err(usage)?;
    Ok(Command::Allow { ask: Allow { name, networks, certificate }, progress })
}

/// The options of `deny`.
fn deny_options(args: &mut Args) -> Result<Command, Fail> {
    let (mut name, mut networks, mut progress) = (None, Vec::new(), false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "from" => networks.push(network(&args.text()?)?),
            Arg::Long(option) if option == "progress" => progress = true,
            Arg::Operand(operand) => take_name(&mut name, operand)?,
            other => return Err(args.unknown(&other)),
        }
    }
    let name = checked_name(name)?;
    names::check_networks(&networks).map_err(usage)?;
    let networks = (!networks.is_empty()).then_some(networks);
    Ok(Command::Deny { ask: Deny { name, networks }, progress })
}

/// The options of `remove`.
fn remove_options(args: &mut Args) -> Result<Command, Fail> {
    let (mut name, mut role, mut force) = (None, false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "role" => role = true,
            Arg::Long(option) if option == "force" => force = true,
            Arg::Operand(operand) => take_name(&mut name, operand)?,
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(Command::Remove(Remove { name: checked_name(name)?, role, force }))
}

/// The options of `copies`.
fn copies_options(args: &mut Args) -> Result<Command, Fail> {
    let (mut name, mut json) = (None, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "json" => json = true,
            Arg::Operand(operand) => take_name(&mut name, operand)?,
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(Command::Copies { name: checked_name(name)?, json })
}

/// The options of `restore`.
fn restore_options(args: &mut Args) -> Result<Command, Fail> {
    let (mut name, mut from, mut new) = (None, None, None);
    let (mut over, mut force, mut progress, mut json) = (false, false, false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "from" => from = Some(args.text()?),
            Arg::Long(option) if option == "as" => new = Some(args.text()?),
            Arg::Long(option) if option == "over" => over = true,
            Arg::Long(option) if option == "force" => force = true,
            Arg::Long(option) if option == "progress" => progress = true,
            Arg::Long(option) if option == "json" => json = true,
            Arg::Operand(operand) => take_name(&mut name, operand)?,
            other => return Err(args.unknown(&other)),
        }
    }
    let name = checked_name(name)?;
    let from =
        from.ok_or_else(|| usage("restore needs --from FILE, the copy to put back".into()))?;
    if !from.starts_with('/') {
        return Err(usage(format!("--from needs the full path of the copy, from /: `{from}`")));
    }
    let target = match (new, over) {
        (Some(_), true) => {
            return Err(usage("--as and --over are two ways to restore: use one".into()));
        }
        (Some(new), false) => {
            names::check_name(&new).map_err(usage)?;
            Target::New(Some(new))
        }
        (None, true) => Target::Over { force },
        (None, false) => Target::New(None),
    };
    if force && !over {
        return Err(usage(
            "--force ends the sessions that --over finds open; it is for --over".into(),
        ));
    }
    Ok(Command::Restore { ask: Restore { name, from, target }, progress, json })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(words: &[&str]) -> Result<Parsed, Fail> {
        let mut args = Args::new("database", words.iter().map(OsString::from).collect());
        parse(&mut args)
    }

    fn command(words: &[&str]) -> Command {
        match parsed(words).unwrap() {
            Parsed::Run(command) => command,
            Parsed::Help => panic!("help"),
        }
    }

    fn refused(words: &[&str]) -> String {
        let failure = parsed(words).unwrap_err();
        assert!(failure.usage, "{}", failure.message);
        failure.message
    }

    #[test]
    fn help_and_a_command_that_is_not_one_are_told() {
        assert_eq!(parsed(&["--help"]).unwrap(), Parsed::Help);
        assert!(refused(&[]).starts_with("a command is needed"));
        assert!(refused(&["drop", "x"]).starts_with("no such command: drop"));
        assert!(refused(&["list", "--nope"]).contains("unrecognized option '--nope'"));
        assert!(refused(&["list", "x"]).contains("unexpected argument 'x'"));
    }

    #[test]
    fn list_has_json() {
        assert_eq!(command(&["list"]), Command::List { json: false });
        assert_eq!(command(&["list", "--json"]), Command::List { json: true });
    }

    #[test]
    fn add_has_the_database_the_role_the_sorting_and_the_site() {
        assert_eq!(
            command(&["add", "shop", "--json"]),
            Command::Add {
                ask: Add {
                    name: "shop".into(),
                    owner: "shop".into(),
                    peer: None,
                    locale: "en_US.UTF-8".into(),
                    site: None
                },
                json: true
            }
        );
        assert_eq!(
            command(&[
                "add",
                "shop",
                "--owner",
                "boss",
                "--locale",
                "C.UTF-8",
                "--site",
                "shop.example.org",
                "--json"
            ]),
            Command::Add {
                ask: Add {
                    name: "shop".into(),
                    owner: "boss".into(),
                    peer: None,
                    locale: "C.UTF-8".into(),
                    site: Some("shop.example.org".into())
                },
                json: true
            }
        );
    }

    #[test]
    fn an_account_is_the_owner_and_has_no_password_to_show() {
        let Command::Add { ask, json } = command(&["add", "notes", "--peer", "webdev"]) else {
            panic!("add")
        };
        assert_eq!(
            (ask.owner.as_str(), ask.peer.as_deref(), json),
            ("webdev", Some("webdev"), false)
        );
        assert!(
            refused(&["add", "notes", "--peer", "webdev", "--owner", "other"])
                .contains("--peer webdev makes the role webdev")
        );
        assert!(matches!(
            command(&["add", "notes", "--peer", "webdev", "--owner", "webdev"]),
            Command::Add { .. }
        ));
    }

    #[test]
    fn a_password_is_only_ever_made_for_json() {
        assert!(refused(&["add", "shop"]).contains("run it with --json"));
        assert!(refused(&["password", "shop"]).contains("run it with --json"));
        assert_eq!(
            command(&["password", "shop", "--json"]),
            Command::Password { name: "shop".into() }
        );
    }

    #[test]
    fn the_names_the_sorting_and_the_site_are_checked_before_anything_is_made() {
        for words in [
            &["add", "Shop", "--json"][..],
            &["add", "pg_x", "--json"],
            &["add", "postgres", "--json"],
            &["add", "a;b", "--json"],
            &["add", "x", "--json", "--owner", "Boss"],
            &["add", "x", "--json", "--locale", "de_DE.UTF-8"],
            &["add", "x", "--json", "--site", "Not A Site"],
            &["add", "--json"],
            &["add", "a", "b", "--json"],
            &["password", "template0", "--json"],
            &["remove", "a b"],
            &["allow", "Shop", "--from", "10.0.0.0/8"],
            &["deny", "pg_x"],
        ] {
            refused(words);
        }
    }

    #[test]
    fn allow_has_networks_and_a_certificate() {
        let Command::Allow { ask, progress } = command(&[
            "allow",
            "shop",
            "--from",
            "192.168.1.0/24",
            "--from",
            "2001:db8::/64",
            "--progress",
        ]) else {
            panic!("allow")
        };
        assert!(progress);
        assert_eq!(ask.certificate, "this-machine");
        assert_eq!(
            ask.networks.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["192.168.1.0/24", "2001:db8::/64"]
        );
        let Command::Allow { ask, .. } = command(&[
            "allow",
            "shop",
            "--from",
            "10.0.2.0/24",
            "--certificate",
            "nas.example.org",
        ]) else {
            panic!("allow")
        };
        assert_eq!(ask.certificate, "nas.example.org");
    }

    #[test]
    fn allow_refuses_what_is_not_a_network_or_a_certificate() {
        assert!(refused(&["allow", "shop"]).contains("at least one --from"));
        assert!(refused(&["allow", "shop", "--from", "10.0.2.0"]).contains("prefix length"));
        assert!(refused(&["allow", "shop", "--from", "anywhere"]).contains("prefix length"));
        assert!(
            refused(&["allow", "shop", "--from", "10.0.2.0/24", "--from", "10.0.2.5/24"])
                .contains("named twice")
        );
        assert!(
            refused(&["allow", "shop", "--from", "10.0.2.0/24", "--certificate", "../x"])
                .contains("certificate's name")
        );
        let many: Vec<String> = (0..21).map(|n| format!("10.0.{n}.0/24")).collect();
        let mut words = vec!["allow", "shop"];
        for network in &many {
            words.push("--from");
            words.push(network);
        }
        assert!(refused(&words).contains("at most 20"));
    }

    #[test]
    fn deny_has_the_networks_or_all_of_them() {
        assert_eq!(
            command(&["deny", "shop"]),
            Command::Deny { ask: Deny { name: "shop".into(), networks: None }, progress: false }
        );
        let Command::Deny { ask, progress } =
            command(&["deny", "shop", "--from", "10.0.2.0/24", "--progress"])
        else {
            panic!("deny")
        };
        assert!(progress);
        assert_eq!(ask.networks.unwrap().len(), 1);
    }

    #[test]
    fn remove_has_the_role_and_force() {
        assert_eq!(
            command(&["remove", "shop"]),
            Command::Remove(Remove { name: "shop".into(), role: false, force: false })
        );
        assert_eq!(
            command(&["remove", "shop", "--role", "--force"]),
            Command::Remove(Remove { name: "shop".into(), role: true, force: true })
        );
    }

    #[test]
    fn copies_has_json() {
        assert_eq!(
            command(&["copies", "shop"]),
            Command::Copies { name: "shop".into(), json: false }
        );
        assert_eq!(
            command(&["copies", "shop", "--json"]),
            Command::Copies { name: "shop".into(), json: true }
        );
        assert!(refused(&["copies"]).contains("the name of a database is needed"));
        assert!(refused(&["copies", "Shop"]).contains("not a name"));
        assert!(refused(&["copies", "a", "b"]).contains("unexpected argument 'b'"));
    }

    #[test]
    fn restore_is_into_a_new_database_unless_it_is_over() {
        let Command::Restore { ask, progress, json } =
            command(&["restore", "shop", "--from", "/shared/x/shop.dump"])
        else {
            panic!("restore")
        };
        assert_eq!(
            ask,
            Restore {
                name: "shop".into(),
                from: "/shared/x/shop.dump".into(),
                target: Target::New(None)
            }
        );
        assert!(!progress && !json);
        let Command::Restore { ask, progress, json } = command(&[
            "restore",
            "shop",
            "--from",
            "/a/shop.dump",
            "--as",
            "shop_old",
            "--progress",
            "--json",
        ]) else {
            panic!("restore")
        };
        assert_eq!(ask.target, Target::New(Some("shop_old".into())));
        assert!(progress && json);
        for (words, force) in [
            (&["restore", "shop", "--from", "/a.dump", "--over"][..], false),
            (&["restore", "shop", "--from", "/a.dump", "--over", "--force"][..], true),
        ] {
            let Command::Restore { ask, .. } = command(words) else { panic!("restore") };
            assert_eq!(ask.target, Target::Over { force });
        }
    }

    #[test]
    fn a_restore_that_is_not_asked_whole_is_refused_before_anything_is_done() {
        let base = ["restore", "shop", "--from", "/a/shop.dump"];
        let with = |more: &[&'static str]| -> String {
            let words: Vec<&str> = base.iter().chain(more).copied().collect();
            refused(&words)
        };
        assert!(with(&["--as", "x", "--over"]).contains("use one"));
        assert!(with(&["--force"]).contains("it is for --over"));
        assert!(with(&["--as", "x", "--force"]).contains("it is for --over"));
        assert!(with(&["--as", "Shop"]).contains("not a name"));
        assert!(with(&["--as", "pg_x"]).contains("keeps for itself"));
        assert!(with(&["--nope"]).contains("unrecognized option '--nope'"));
        assert!(refused(&["restore", "shop"]).contains("needs --from FILE"));
        assert!(refused(&["restore", "--from", "/a.dump"]).contains("the name of a database"));
        assert!(refused(&["restore", "shop", "--from", "a.dump"]).contains("the full path"));
        assert!(refused(&["restore", "Shop", "--from", "/a.dump"]).contains("not a name"));
        assert!(refused(&["restore", "shop", "x", "--from", "/a.dump"]).contains("unexpected"));
    }

    #[test]
    fn the_help_names_every_command_and_option() {
        for word in [
            "list",
            "add",
            "password",
            "allow",
            "deny",
            "remove",
            "copies",
            "restore",
            "--as",
            "--over",
            "--json",
            "--progress",
            "--role",
            "--force",
            "--peer",
            "--site",
            "--from",
            "--certificate",
        ] {
            assert!(HELP.contains(word), "{word}");
        }
    }
}
