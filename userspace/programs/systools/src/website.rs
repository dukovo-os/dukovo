// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! website(8): nginx's websites, added, taken to HTTPS, listed, checked and
//! removed, for the web console and for people at a terminal (ADR 0569).
//!
//! A website is a file, `/system/settings/websites/NAME.conf`, which
//! nginx's settings include, and a folder under `/shared/www/sites`.
//! `website` writes the file from a template, so that what it writes is
//! known and a file a person changed is told from one it wrote, and keeps
//! the keys of nginx's service file that the sites need: the
//! certificates, the sockets for port 443 and the folder certd answers
//! from.
//!
//! What a person types reaches a file only after it passes the rule for
//! its kind: a name by the certificates' rule, a folder by the characters
//! `website` writes unquoted, an account by the accounts' rule. Every
//! change is checked before anything changes, nginx's own check runs on
//! the new set of files before it is live, each file is written whole and
//! renamed into place, and a step that fails takes the earlier ones back.
//!
//! The modules: [`conf`] reads nginx's syntax; [`plan`] makes the files
//! and the keys; [`change`] and [`remove`] are the changes, [`preview`] the
//! plan of an add, and [`list`] what `list` and `check` say; [`count`]
//! counts each site's requests and the size of its folder once a day;
//! [`host`] is what is asked of the machine, [`folder`] the sites' folders,
//! [`stage`] nginx's check of a new set and the lock, and [`answer`] the
//! check that the site answers.

mod answer;
mod change;
mod conf;
mod count;
mod folder;
mod host;
mod list;
mod plan;
mod preview;
mod remove;
mod stage;

#[cfg(test)]
mod fake;

use std::io::{self, Write};

use change::{AddRequest, Ctx, Https};
use conf::Kind;
use host::{Host, Paths, System};

pub use change::OVERRIDE;
pub use plan::{DEFAULT_FILE, SETTINGS};

use crate::args::{Arg, Args};
use crate::cert::acme::steps::Steps;
use crate::util::{self, Fail, fail, usage};

/// What a mistake in the command line is told.
const USAGE: &str = "usage: website [list] [--json]\n       website check [--json]\n       \
    website add NAME [--name DNS]... [--kind static|php] [--folder FOLDER]\n                    \
    --owner ACCOUNT [--certificate NAME] [--progress]\n       \
    website add NAME ... --dry-run [--json]\n       \
    website https NAME --certificate NAME [--progress]\n       \
    website https NAME --off [--progress]\n       \
    website remove NAME [--files] [--progress]\n       \
    website count [--now] [--json]";

/// The most names a website answers to.
pub const NAMES_MAX: usize = 20;

/// The longest name a site has: its file's name, `NAME.conf`, must fit
/// the 255 bytes a file's name may have.
pub const NAME_MAX: usize = 250;

/// The most bytes of the path of a site's folder.
pub const FOLDER_MAX: usize = 1024;

/// How many steps `add` tells.
const ADD_STEPS: usize = 7;
/// How many steps `https` tells.
const HTTPS_STEPS: usize = 6;
/// How many steps `remove` tells.
const REMOVE_STEPS: usize = 5;

/// What was asked.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    List { json: bool },
    Check { json: bool },
    Add { request: AddRequest, dry_run: bool, json: bool, progress: bool },
    Https { name: String, how: Https, progress: bool },
    Remove { name: String, files: bool, progress: bool },
    Count { now: bool, json: bool },
}

impl Command {
    /// Whether only root may ask it.
    fn needs_root(&self) -> bool {
        !matches!(self, Command::Help | Command::List { .. })
    }
}

/// Checks that `name` is a name a website answers to: a name by the rule
/// certificates follow, lowercase, with one label allowed, no `*.` and no
/// address by number.
///
/// # Errors
///
/// What is wrong with it.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.starts_with("*.") {
        return Err(format!(
            "`{}` is a wildcard: a website answers to each of its names, and a certificate may \
             hold a wildcard that covers them",
            name.escape_debug()
        ));
    }
    if name.parse::<std::net::IpAddr>().is_ok() {
        return Err(format!("{name} is an address by number: a website answers to a name"));
    }
    crate::cert::acme::check_dns_name_to_check(name, false).map_err(|_| {
        format!(
            "`{}` is not a name a website answers to: it has lowercase letters, digits and `-` \
             in labels of up to 63 characters, with a dot between them, at most 253 characters \
             in all; write a name that is not ASCII in punycode, as xn--...",
            name.escape_debug()
        )
    })
}

/// Checks `folder`, a site's folder, as `website add --folder` does: under
/// the sites' folder, in parts that `website` writes into a file as they
/// are.
///
/// # Errors
///
/// What is wrong with it.
pub fn check_folder(folder: &str) -> Result<(), String> {
    folder::parts(folder).map(|_| ())
}

/// Checks the name of a site, which is also its file's name.
///
/// # Errors
///
/// What is wrong with it.
pub fn check_site_name(name: &str) -> Result<(), String> {
    check_name(name)?;
    if name.len() > NAME_MAX {
        return Err(format!(
            "`{name}` is too long for a site's name, which is its file's too: {} characters at \
             most",
            NAME_MAX
        ));
    }
    Ok(())
}

/// The text of the operand or option value `value`.
fn text(value: std::ffi::OsString) -> Result<String, Fail> {
    value.into_string().map_err(|bad| usage(format!("'{}' is not UTF-8", bad.to_string_lossy())))
}

/// Reads what `website add` was asked.
fn parse_add(args: &mut Args) -> Result<Command, Fail> {
    let (mut site, mut more, mut kind, mut folder) = (None, Vec::new(), None, None);
    let (mut owner, mut certificate) = (None, None);
    let (mut dry_run, mut json, mut progress) = (false, false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if site.is_none() => site = Some(text(operand)?),
            Arg::Long(name) if name == "name" => more.push(args.text()?),
            Arg::Long(name) if name == "kind" => kind = Some(args.text()?),
            Arg::Long(name) if name == "folder" => folder = Some(args.text()?),
            Arg::Long(name) if name == "owner" => owner = Some(args.text()?),
            Arg::Long(name) if name == "certificate" => certificate = Some(args.text()?),
            Arg::Long(name) if name == "dry-run" => dry_run = true,
            Arg::Long(name) if name == "json" => json = true,
            Arg::Long(name) if name == "progress" => progress = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(first) = site else {
        return Err(usage(format!("website add needs the site's name\n{USAGE}")));
    };
    // The site's name comes first, whatever the order it was typed in:
    // options are read before the operand is.
    check_site_name(&first).map_err(usage)?;
    for name in &more {
        check_name(name).map_err(usage)?;
    }
    let mut names = vec![first];
    names.extend(more);
    if names.len() > NAMES_MAX {
        return Err(usage(format!("a website answers to at most {NAMES_MAX} names")));
    }
    let mut seen = names.clone();
    seen.sort();
    if let Some(twice) = seen.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(usage(format!("{} is named twice", twice[0])));
    }
    let kind = match kind.as_deref() {
        None | Some("static") => Kind::Static,
        Some("php") => Kind::Php,
        Some(other) => return Err(usage(format!("a site's kind is static or php, not {other}"))),
    };
    let folder = folder.unwrap_or_else(|| format!("{}/{}", plan::SITES, names[0]));
    folder::parts(&folder).map_err(usage)?;
    let Some(owner) = owner else {
        return Err(usage(format!(
            "website add needs --owner, the account the folder belongs to\n{USAGE}"
        )));
    };
    manifest::service::check_account(&owner).map_err(usage)?;
    if let Some(certificate) = &certificate {
        manifest::service::check_certificate(certificate).map_err(usage)?;
    }
    if json && !dry_run {
        return Err(usage("--json goes with --dry-run, which prints the plan".into()));
    }
    if dry_run && progress {
        return Err(usage(
            "--dry-run changes nothing, so it has no steps: leave out --progress".into(),
        ));
    }
    let request = AddRequest { names, kind, folder, owner, certificate };
    Ok(Command::Add { request, dry_run, json, progress })
}

/// Reads what `website https` was asked.
fn parse_https(args: &mut Args) -> Result<Command, Fail> {
    let (mut name, mut certificate, mut off, mut progress) = (None, None, false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if name.is_none() => name = Some(text(operand)?),
            Arg::Long(option) if option == "certificate" => certificate = Some(args.text()?),
            Arg::Long(option) if option == "off" => off = true,
            Arg::Long(option) if option == "progress" => progress = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(name) = name else {
        return Err(usage(format!("website https needs the site's name\n{USAGE}")));
    };
    check_site_name(&name).map_err(usage)?;
    let how = match (certificate, off) {
        (Some(certificate), false) => {
            manifest::service::check_certificate(&certificate).map_err(usage)?;
            Https::With(certificate)
        }
        (None, true) => Https::Off,
        _ => {
            return Err(usage(
                "website https takes --certificate NAME or --off, one of them".into(),
            ));
        }
    };
    Ok(Command::Https { name, how, progress })
}

/// Reads what `website remove` was asked.
fn parse_remove(args: &mut Args) -> Result<Command, Fail> {
    let (mut name, mut files, mut progress) = (None, false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if name.is_none() => name = Some(text(operand)?),
            Arg::Long(option) if option == "files" => files = true,
            Arg::Long(option) if option == "progress" => progress = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(name) = name else {
        return Err(usage(format!("website remove needs the site's name\n{USAGE}")));
    };
    check_site_name(&name).map_err(usage)?;
    Ok(Command::Remove { name, files, progress })
}

/// Reads the options of `list` and `check`: `--json` and `--help`.
fn parse_read(args: &mut Args) -> Result<(bool, bool), Fail> {
    let (mut json, mut help) = (false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "json" => json = true,
            Arg::Long(option) if option == "help" => help = true,
            other => return Err(args.unknown(&other)),
        }
    }
    Ok((json, help))
}

/// Reads what `website count` was asked.
fn parse_count(args: &mut Args) -> Result<Command, Fail> {
    let (mut now, mut json, mut help) = (false, false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "now" => now = true,
            Arg::Long(option) if option == "json" => json = true,
            Arg::Long(option) if option == "help" => help = true,
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(if help { Command::Help } else { Command::Count { now, json } })
}

/// Reads what was asked.
fn parse(args: &mut Args) -> Result<Command, Fail> {
    let subcommand = args.subcommand();
    let word = subcommand.as_deref().map(|word| word.to_str());
    match word {
        None | Some(Some("list")) => {
            let (json, help) = parse_read(args)?;
            Ok(if help { Command::Help } else { Command::List { json } })
        }
        Some(Some("check")) => {
            let (json, help) = parse_read(args)?;
            Ok(if help { Command::Help } else { Command::Check { json } })
        }
        Some(Some("add")) => parse_add(args),
        Some(Some("https")) => parse_https(args),
        Some(Some("remove")) => parse_remove(args),
        Some(Some("count")) => parse_count(args),
        _ => Err(usage(format!(
            "no such command: {}\n{USAGE}",
            subcommand.unwrap_or_default().to_string_lossy()
        ))),
    }
}

/// Runs `command` on `paths` and `host`, writing what it says to `out`.
fn execute(
    command: Command,
    paths: &Paths,
    host: &dyn Host,
    out: &mut dyn Write,
) -> Result<i32, Fail> {
    let off = Steps::off();
    let tell = |out: &mut dyn Write, lines: &[String]| -> io::Result<()> {
        for line in lines {
            writeln!(out, "{line}")?;
        }
        Ok(())
    };
    match command {
        Command::Help => {
            writeln!(out, "{USAGE}")?;
            Ok(0)
        }
        Command::List { json } => {
            let listing = list::list(&Ctx { paths, host, steps: &off }).map_err(|refusal| {
                let mut fail = refusal.into_fail();
                if fail.message.contains("Permission denied") {
                    fail.message.push_str(": only root reads that, so elevate first");
                }
                fail
            })?;
            if json {
                writeln!(out, "{}", listing.json().map_err(fail)?)?;
            } else {
                write!(out, "{}", listing.plain())?;
            }
            Ok(0)
        }
        Command::Check { json } => {
            let (report, ok) = list::check(&Ctx { paths, host, steps: &off })
                .map_err(change::Refusal::into_fail)?;
            if json {
                writeln!(out, "{}", report.json().map_err(fail)?)?;
            } else {
                write!(out, "{}", report.plain())?;
            }
            Ok(i32::from(!ok))
        }
        Command::Add { request, dry_run: true, json, .. } => {
            let (plan, passed) = preview::plan(&Ctx { paths, host, steps: &off }, &request)
                .map_err(change::Refusal::into_fail)?;
            if json {
                writeln!(out, "{}", plan.json().map_err(fail)?)?;
            } else {
                write!(out, "{}", plan.plain())?;
            }
            Ok(i32::from(!passed))
        }
        Command::Add { request, progress, .. } => {
            let steps = if progress { Steps::new(ADD_STEPS) } else { Steps::off() };
            let lines = change::add(&Ctx { paths, host, steps: &steps }, &request)
                .map_err(change::Refusal::into_fail)?;
            tell(out, &lines)?;
            Ok(0)
        }
        Command::Https { name, how, progress } => {
            let steps = if progress { Steps::new(HTTPS_STEPS) } else { Steps::off() };
            let lines = change::https(&Ctx { paths, host, steps: &steps }, &name, &how)
                .map_err(change::Refusal::into_fail)?;
            tell(out, &lines)?;
            Ok(0)
        }
        Command::Remove { name, files, progress } => {
            let total = REMOVE_STEPS + usize::from(files);
            let steps = if progress { Steps::new(total) } else { Steps::off() };
            let lines = remove::remove(&Ctx { paths, host, steps: &steps }, &name, files)
                .map_err(change::Refusal::into_fail)?;
            tell(out, &lines)?;
            Ok(0)
        }
        Command::Count { now, json } => {
            let report = count::count(&Ctx { paths, host, steps: &off }, now, count::Limits::STANDARD)
                .map_err(change::Refusal::into_fail)?;
            if json {
                writeln!(out, "{}", report.json().map_err(fail)?)?;
            } else {
                write!(out, "{}", report.plain())?;
            }
            Ok(0)
        }
    }
}

/// website(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let command = parse(args)?;
    if command.needs_root() && util::own_user() != 0 {
        return Err(fail(
            "only root changes websites and asks nginx about them: elevate first, as `sudo website ...`"
                .into(),
        ));
    }
    // The daily count is work that can wait. One asked for is not.
    if matches!(command, Command::Count { now: false, .. })
        && let Err(why) = crate::bundles::sizes::lower_priority()
    {
        eprintln!("website: {why}");
    }
    let mut out = io::stdout().lock();
    execute(command, &Paths::system(), &System, &mut out)
}

#[cfg(test)]
mod tests;
