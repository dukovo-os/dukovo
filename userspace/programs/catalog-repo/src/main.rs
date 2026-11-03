// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! catalog-repo: makes and signs the catalog's TUF repository (ADR 0381).
//! `ob catalog` builds it for the build machine and runs it with the
//! source tree's paths; ob(1) says what each command does.

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use catalog_repo::keys::{self, Signers};
use catalog_repo::{Repo, WARN_DAYS, expires_days};

const USAGE: &str = "\
usage: catalog-repo --repo DIR [--yubikey-helper PROGRAM] [--key FILE]... COMMAND
       key yubikey [--serial N]          add the key of a YubiKey's slot 9c
       key new FILE NAME [ROLE]...       make an Ed25519 key, its secret in FILE
       root [--days N]                   sign the next root, from keys/
       sign [--days N] [--remove PATH]... [--yes] [BUNDLE]...
                                         sign the next targets: one touch a batch,
                                         the bundles in --bundles DIR by default
       publish --store DIR --out DIR     snapshot, timestamp and the files to serve
       status                            the roles' versions, expiry and keys";

struct Options {
    repo: Option<PathBuf>,
    helper: Option<PathBuf>,
    keys: Vec<PathBuf>,
    days: Option<i64>,
    serial: Option<u64>,
    remove: Vec<String>,
    yes: bool,
    store: Option<PathBuf>,
    out: Option<PathBuf>,
    bundles: Option<PathBuf>,
    rest: Vec<String>,
}

fn parse(args: Vec<String>) -> Result<Options, String> {
    let mut options = Options {
        repo: None,
        helper: None,
        keys: Vec::new(),
        days: None,
        serial: None,
        remove: Vec::new(),
        yes: false,
        store: None,
        out: None,
        bundles: None,
        rest: Vec::new(),
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--repo" => options.repo = Some(value()?.into()),
            "--yubikey-helper" => options.helper = Some(value()?.into()),
            "--key" => options.keys.push(value()?.into()),
            "--days" => options.days = Some(value()?.parse().map_err(|_| "--days takes a number")?),
            "--serial" => {
                options.serial = Some(value()?.parse().map_err(|_| "--serial takes a number")?);
            }
            "--remove" => options.remove.push(value()?),
            "--store" => options.store = Some(value()?.into()),
            "--out" => options.out = Some(value()?.into()),
            "--bundles" => options.bundles = Some(value()?.into()),
            "--yes" => options.yes = true,
            "-h" | "--help" => return Err(USAGE.into()),
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}\n{USAGE}")),
            _ => options.rest.push(arg),
        }
    }
    Ok(options)
}

fn confirm(question: &str) -> Result<bool, String> {
    if !std::io::stdin().is_terminal() {
        return Err(format!("{question} needs an answer: give --yes"));
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

fn run(options: &Options) -> Result<(), String> {
    let repo = Repo::new(options.repo.as_ref().ok_or(format!("--repo is needed\n{USAGE}"))?);
    let signers = || Signers::new(options.helper.as_deref(), &options.keys);
    let words: Vec<&str> = options.rest.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["key", "yubikey"] => {
            let helper = options.helper.as_ref().ok_or("--yubikey-helper is needed")?;
            let key = keys::from_yubikey(helper, options.serial)?;
            let path = key.write(&repo.keys_dir())?;
            println!(
                "{}: key {} of YubiKey {}",
                path.display(),
                key.key.id(),
                key.yubikey.unwrap_or(0)
            );
        }
        ["key", "new", file, name, roles @ ..] => {
            let roles: Vec<String> = if roles.is_empty() {
                vec!["snapshot".into(), "timestamp".into()]
            } else {
                roles.iter().map(|r| (*r).to_string()).collect()
            };
            if let Some(bad) = roles.iter().find(|r| !catalog_repo::ROLES.contains(&r.as_str())) {
                return Err(format!("{bad} is not a role"));
            }
            let key = keys::new_file_key(name, roles, &PathBuf::from(file))?;
            let path = key.write(&repo.keys_dir())?;
            println!("{}: key {}; its secret half is in {file}", path.display(), key.key.id());
        }
        ["root"] => {
            let version =
                repo.sign_root(&signers()?, options.days.unwrap_or(expires_days("root")))?;
            println!("Signed root version {version}.");
        }
        ["sign", bundles @ ..] => {
            let mut bundles: Vec<PathBuf> = bundles.iter().map(PathBuf::from).collect();
            if bundles.is_empty()
                && let Some(dir) = &options.bundles
                && let Ok(entries) = std::fs::read_dir(dir)
            {
                bundles = entries
                    .filter_map(|entry| Some(entry.ok()?.path()))
                    .filter(|path| path.extension().is_some_and(|e| e == dkv::EXTENSION))
                    .collect();
                bundles.sort();
            }
            let changes = repo.changes(&bundles, &options.remove)?;
            let store = options.store.as_ref().ok_or("--store is needed")?;
            if changes.is_empty() {
                println!("Nothing has changed; signing again renews targets' expiry.");
            }
            for line in changes.lines() {
                println!("{line}");
            }
            if !options.yes && !confirm("Sign these into the catalog?")? {
                return Err("not signed".into());
            }
            let days = options.days.unwrap_or(expires_days("targets"));
            let version = repo.sign_targets(&changes, &bundles, store, &signers()?, days)?;
            println!("Signed targets version {version}.");
        }
        ["publish"] => {
            let store = options.store.as_ref().ok_or("--store is needed")?;
            let out = options.out.as_ref().ok_or("--out is needed")?;
            let days = (expires_days("snapshot"), expires_days("timestamp"));
            let (snapshot, timestamp) = repo.publish(store, out, &signers()?, days)?;
            println!(
                "Published snapshot {snapshot} and timestamp {timestamp} in {}.",
                out.display()
            );
        }
        ["status"] => status(&repo)?,
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

fn status(repo: &Repo) -> Result<(), String> {
    let now = tuf::time::now();
    let line = |role: &str, version: u64, expires: i64| {
        let days = (expires - now).div_euclid(86_400);
        let warning = if expires <= now {
            " EXPIRED: sign it again"
        } else if days < WARN_DAYS {
            " (soon: sign it again)"
        } else {
            ""
        };
        println!(
            "{role}: version {version}, expires {} ({days} days){warning}",
            tuf::time::format(expires)
        );
    };
    match repo.root()? {
        Some(root) => line("root", root.signed.version, root.signed.expires),
        None => println!("root: none yet"),
    }
    match repo.targets()? {
        Some(targets) => {
            line("targets", targets.signed.version, targets.signed.expires);
            println!("  {} bundles listed", targets.signed.signed.targets.len());
        }
        None => println!("targets: none yet"),
    }
    for key in keys::read_all(&repo.keys_dir())? {
        let holder = key.yubikey.map_or("a file".to_string(), |serial| format!("YubiKey {serial}"));
        println!(
            "key {}: {} on {holder}, for {}",
            key.name,
            &key.key.id()[..16],
            key.roles.join(", ")
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let result = parse(std::env::args().skip(1).collect()).and_then(|options| run(&options));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("catalog-repo: {err}");
            ExitCode::FAILURE
        }
    }
}
