// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! catalog-repo: makes and signs the catalog's TUF repository (ADR 0381).
//! `ob catalog` builds it for the build machine and runs it with the
//! source tree's paths; ob(1) says what each command does.

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};

use catalog_repo::keys::{self, Signers};
use catalog_repo::plan::{self, Action};
use catalog_repo::{Changes, FLAWS_DAYS, Refreshed, Repo, WARN_DAYS, expires_days};
use system_release::rollout::{Plan, Plans};
use system_release::verdict;

const USAGE: &str = "\
usage: catalog-repo --repo DIR [--yubikey-helper PROGRAM] [--key FILE]... COMMAND
       key yubikey [--serial N]          add the key of a YubiKey's slot 9c
       key new FILE NAME [ROLE]...       make an Ed25519 key, its secret in FILE
       root [--days N]                   sign the next root, from keys/
       sign [--days N] [--remove PATH]... [--verified ID]... [--renew]
            [--yes | --preview FILE | --expect DIGEST] [BUNDLE]...
                                         sign the next targets: one touch a batch,
                                         the bundles in --bundles DIR by default;
                                         a bundle of an ID given with --verified is
                                         listed as one whose publisher the catalog
                                         vouches for; a version listed already as
                                         other bytes is kept as signed; --renew
                                         reads no bundle; --preview writes the plan
                                         to FILE and signs nothing; --expect signs
                                         only a plan of that digest
       withdraw --id ID --version V --kind broken|malicious --reason TEXT
                [--days N] [--yes | --preview FILE | --expect DIGEST] --store DIR
                                         sign the next targets with that version of
                                         ID, as the targets list it, withdrawn
       since [FILE]                      what targets.json changed since the targets
                                         in FILE, or since none
       verify                            check the catalog offline, as a computer
                                         checks it
       publish --store DIR --out DIR [--plan FILE]
                                         snapshot, timestamp and the files to serve,
                                         the rollouts of the plan signed first
       refresh --store DIR --out DIR [--plan FILE]
                                         a new timestamp, or all again when needed
       rollout --store DIR --out DIR --plan FILE
               (pause ID VERSION REASON | resume ID VERSION
                | canaries --canaries FILE --reports DIR)
                                         hold or free the waves of a bundle's newest
                                         version, or of every one, and sign again; with
                                         canaries, hold them while one is not green
       canaries changed --canaries FILE --reports DIR --plan FILE
                                         exit 0 when the canaries not green are not those
                                         the plan holds the waves for, 3 when they are
       flaws --store DIR --out DIR OSV...
                                         sign the list of the bundles' known flaws
                                         from advisories in the OSV format, and publish
       check URL                         read what URL serves from the first root
       served [--json] URL               what URL serves, read from the first root as
                                         a computer does, without fetching the bundles
       status [--json]                    the roles' versions, expiry and keys";

struct Options {
    repo: Option<PathBuf>,
    helper: Option<PathBuf>,
    keys: Vec<PathBuf>,
    days: Option<i64>,
    serial: Option<u64>,
    remove: Vec<String>,
    verified: Vec<String>,
    id: Option<String>,
    version: Option<String>,
    kind: Option<String>,
    reason: Option<String>,
    yes: bool,
    renew: bool,
    preview: Option<PathBuf>,
    expect: Option<String>,
    json: bool,
    store: Option<PathBuf>,
    out: Option<PathBuf>,
    bundles: Option<PathBuf>,
    plan: Option<PathBuf>,
    canaries: Option<PathBuf>,
    reports: Option<PathBuf>,
    /// For tests: the time the canaries are judged at, in place of now.
    now: Option<i64>,
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
        verified: Vec::new(),
        id: None,
        version: None,
        kind: None,
        reason: None,
        yes: false,
        renew: false,
        preview: None,
        expect: None,
        json: false,
        store: None,
        out: None,
        bundles: None,
        plan: None,
        canaries: None,
        reports: None,
        now: None,
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
            "--verified" => options.verified.push(value()?),
            "--id" => options.id = Some(value()?),
            "--version" => options.version = Some(value()?),
            "--kind" => options.kind = Some(value()?),
            "--reason" => options.reason = Some(value()?),
            "--store" => options.store = Some(value()?.into()),
            "--out" => options.out = Some(value()?.into()),
            "--bundles" => options.bundles = Some(value()?.into()),
            "--plan" => options.plan = Some(value()?.into()),
            "--canaries" => options.canaries = Some(value()?.into()),
            "--reports" => options.reports = Some(value()?.into()),
            "--now" => {
                options.now = Some(value()?.parse().map_err(|_| "--now takes seconds since 1970")?);
            }
            "--preview" => options.preview = Some(value()?.into()),
            "--expect" => options.expect = Some(value()?),
            "--yes" => options.yes = true,
            "--renew" => options.renew = true,
            "--json" => options.json = true,
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

/// What the command did, as the exit status: 0, or, for `canaries changed`,
/// 3 when the rollouts need no signing again.
fn run(options: &Options) -> Result<ExitCode, String> {
    let repo = Repo::new(options.repo.as_ref().ok_or(format!("--repo is needed\n{USAGE}"))?);
    // The YubiKeys are looked for only when a key here is a YubiKey's, so
    // that key files alone, as the update server's, need no yubikey-manager.
    let signers = || {
        let yubikeys = keys::read_all(&repo.keys_dir())?.iter().any(|key| key.yubikey.is_some());
        Signers::new(options.helper.as_deref().filter(|_| yubikeys), &options.keys)
    };
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
        ["sign", named @ ..] => sign(&repo, options, named, signers)?,
        ["withdraw"] => withdraw(&repo, options, signers)?,
        ["since"] => since(&repo, None)?,
        ["since", file] => since(&repo, Some(Path::new(file)))?,
        ["verify"] => println!("{}", repo.verify(tuf::time::now())?.line()),
        ["publish"] => {
            let store = options.store.as_ref().ok_or("--store is needed")?;
            let out = options.out.as_ref().ok_or("--out is needed")?;
            let signers = signers()?;
            let plans = sign_rollouts(&repo, options, &signers)?;
            let days = (expires_days("snapshot"), expires_days("timestamp"));
            let (snapshot, timestamp) = repo.publish(store, out, &signers, days)?;
            keep_plans(options, plans.as_ref())?;
            println!(
                "Published snapshot {snapshot} and timestamp {timestamp} in {}.",
                out.display()
            );
        }
        ["refresh"] => {
            let store = options.store.as_ref().ok_or("--store is needed")?;
            let out = options.out.as_ref().ok_or("--out is needed")?;
            let signers = signers()?;
            let plans = sign_rollouts(&repo, options, &signers)?;
            let days = (expires_days("snapshot"), expires_days("timestamp"));
            let refreshed = repo.refresh(store, out, &signers, days)?;
            keep_plans(options, plans.as_ref())?;
            say_refreshed(&refreshed, out);
        }
        ["rollout", words @ ..] => rollout(&repo, options, &signers()?, words)?,
        ["canaries", "changed"] => {
            let changed = canaries_changed(options)?;
            return Ok(ExitCode::from(if changed { 0 } else { verdict::UNCHANGED_STATUS }));
        }
        ["flaws", sources @ ..] => {
            let store = options.store.as_ref().ok_or("--store is needed")?;
            let out = options.out.as_ref().ok_or("--out is needed")?;
            if sources.is_empty() {
                return Err("name the advisories: OSV's export of an ecosystem, all.zip".into());
            }
            let mut advisories = Vec::new();
            for source in sources {
                let data = std::fs::read(source).map_err(|e| format!("{source}: {e}"))?;
                advisories
                    .extend(dkv::flaws::read_osv(&data).map_err(|e| format!("{source}: {e}"))?);
            }
            let signers = signers()?;
            let days = options.days.unwrap_or(FLAWS_DAYS);
            let (version, count) = repo.sign_flaws(store, &signers, &advisories, days)?;
            println!(
                "Signed flaws version {version}: {count} known flaws, from {} advisories.",
                advisories.len()
            );
            let days = (expires_days("snapshot"), expires_days("timestamp"));
            let (snapshot, timestamp) = repo.publish(store, out, &signers, days)?;
            println!(
                "Published snapshot {snapshot} and timestamp {timestamp} in {}.",
                out.display()
            );
        }
        ["check", url] => {
            let count = repo.check_url(url)?;
            println!(
                "{url}: the client reads it from the first root and fetches all {count} bundles."
            );
        }
        ["served", url] => served(&repo, options, url)?,
        ["status"] if options.json => status_json(&repo, options)?,
        ["status"] => status(&repo)?,
        _ => return Err(USAGE.into()),
    }
    Ok(ExitCode::SUCCESS)
}

/// Whether `--expect` holds a digest: 64 lower-case hex digits.
fn is_digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Says what `plan` changes, then settles whether to sign it: with
/// `--preview`, it writes the plan and signs nothing; with `--expect`, it
/// signs only when the plan's digest is the one given, which is checked
/// before any key is looked for; otherwise it asks, unless `--yes`.
fn settle(
    repo: &Repo,
    options: &Options,
    plan: &plan::Plan,
    days: i64,
    ask: &str,
) -> Result<bool, String> {
    let given = [options.yes, options.preview.is_some(), options.expect.is_some()];
    if given.iter().filter(|given| **given).count() > 1 {
        return Err("give one of --yes, --preview and --expect".into());
    }
    if let Some(expected) = &options.expect
        && !is_digest(expected)
    {
        return Err(format!("--expect takes a plan's digest, 64 hex digits, not {expected:?}"));
    }
    if plan.changes.is_empty() && plan.action != Action::Withdraw {
        println!("Nothing has changed; signing again renews targets' expiry.");
    }
    for line in plan.changes.lines() {
        println!("{line}");
    }
    if let Some(file) = &options.preview {
        let written = repo.write_plan(plan, days, file)?;
        println!(
            "Targets version {} would be signed, good until {}. Nothing is signed yet.",
            written["targets"]["version"],
            written["targets"]["expires"].as_str().unwrap_or("?")
        );
        println!("Plan: {}", file.display());
        return Ok(false);
    }
    if let Some(expected) = &options.expect {
        let digest = plan.digest()?;
        if digest != *expected {
            return Err(format!(
                "what would be signed now is not what was previewed (digest {digest}, previewed \
                 {expected}): prepare again"
            ));
        }
        return Ok(true);
    }
    if !options.yes && !confirm(ask)? {
        return Err("not signed".into());
    }
    Ok(true)
}

/// `sign [BUNDLE]...`: signs the next targets with the bundles named, or
/// those in `--bundles`, or with none when `--renew`.
fn sign(
    repo: &Repo,
    options: &Options,
    named: &[&str],
    signers: impl Fn() -> Result<Signers, String>,
) -> Result<(), String> {
    if options.renew && (!named.is_empty() || !options.remove.is_empty()) {
        return Err(
            "--renew signs the targets as they are: name no bundle and take none out".into()
        );
    }
    let bundles: Vec<PathBuf> = match named {
        _ if options.renew => Vec::new(),
        [] => bundles_in(options.bundles.as_deref()),
        named => named.iter().map(PathBuf::from).collect(),
    };
    let changes =
        repo.changes(&bundles, &options.remove, &options.verified, options.store.as_deref())?;
    changes.rebuilds_say_why()?;
    let action = if options.renew { Action::Renew } else { Action::Sign };
    let plan = repo.plan(action, changes)?;
    let days = options.days.unwrap_or(expires_days("targets"));
    if !settle(repo, options, &plan, days, "Sign these into the catalog?")? {
        return Ok(());
    }
    let store = options.store.as_ref().ok_or("--store is needed")?;
    let version = repo.sign_targets(&plan.changes, &bundles, store, &signers()?, days)?;
    println!("Signed targets version {version}.");
    Ok(())
}

/// `since [FILE]`: the lines of what `targets.json` changed since the
/// targets in FILE, or since none.
fn since(repo: &Repo, file: Option<&Path>) -> Result<(), String> {
    let old = match file {
        Some(file) => {
            let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
            let old = tuf::metadata::Signed::<tuf::metadata::Targets>::parse(&bytes)
                .map_err(|e| format!("{}: {e}", file.display()))?;
            Some(old)
        }
        None => None,
    };
    let new = repo.targets()?.ok_or("no targets have been signed yet")?;
    let changes = Changes::between(old.as_ref().map(|old| &old.signed), &new.signed.signed);
    for line in changes.lines() {
        println!("{line}");
    }
    Ok(())
}

/// Signs the next targets with one version withdrawn, since `--now`.
fn withdraw(
    repo: &Repo,
    options: &Options,
    signers: impl Fn() -> Result<Signers, String>,
) -> Result<(), String> {
    let needed = |value: &Option<String>, name: &str| {
        value.clone().ok_or(format!("{name} is needed\n{USAGE}"))
    };
    let id = needed(&options.id, "--id")?;
    let version = needed(&options.version, "--version")?;
    let kind = needed(&options.kind, "--kind")?;
    let reason = needed(&options.reason, "--reason")?;
    let since = u64::try_from(options.now.unwrap_or_else(tuf::time::now))
        .map_err(|_| "--now takes seconds since 1970")?;
    let changes = repo.withdraw(&id, &version, &kind, &reason, since)?;
    let plan = repo.plan(Action::Withdraw, changes)?;
    let days = options.days.unwrap_or(expires_days("targets"));
    if !settle(repo, options, &plan, days, "Sign this withdrawal into the catalog?")? {
        return Ok(());
    }
    let store = options.store.as_ref().ok_or("--store is needed")?;
    let signed = repo.sign_targets(&plan.changes, &[], store, &signers()?, days)?;
    println!("Signed targets version {signed}.");
    Ok(())
}

/// The canaries that are not green, by their reports in `--reports` at
/// `--now`, and so the names the plan should hold the waves for: the
/// catalog has no rule on a release a canary did not take, which is the
/// system's alone (ADR 0533).
fn canaries_red(options: &Options) -> Result<Vec<String>, String> {
    let canaries = options.canaries.as_ref().ok_or("--canaries is needed")?;
    let reports = options.reports.as_ref().ok_or("--reports is needed")?;
    let now = options.now.unwrap_or_else(tuf::time::now);
    let standings = verdict::standings(canaries, reports, &[], now)?;
    Ok(system_release::canary::red(&standings))
}

/// `canaries changed --canaries FILE --reports DIR --plan FILE`: whether
/// the canaries that are not green are not those the plan holds the
/// waves for, so that the rollouts are to be signed again. It says so in
/// one line, and says nothing when they are the same.
fn canaries_changed(options: &Options) -> Result<bool, String> {
    let path = options.plan.as_ref().ok_or("--plan is needed")?;
    let held = Plans::load(path)?.red;
    let red = canaries_red(options)?;
    if !verdict::differs(&held, &red) {
        return Ok(false);
    }
    println!("{}.", verdict::change_line(&held, &red));
    Ok(true)
}

/// Signs the rollouts of the plans in `--plan`, and gives the plans as
/// they are then, to keep once what they say is published. Without
/// `--plan`, targets that delegate the rollouts still get them, with
/// every wave open, so that computers read the catalog.
fn sign_rollouts(
    repo: &Repo,
    options: &Options,
    signers: &Signers,
) -> Result<Option<Plans>, String> {
    let Some(path) = &options.plan else {
        if let Some(store) = &options.store
            && repo.sign_rollouts(store, signers, None, tuf::time::now())?.is_some()
        {
            eprintln!(
                "catalog-repo: the targets delegate rollouts and no --plan was given: every \
                 bundle's newest version is signed with every wave open"
            );
        }
        return Ok(None);
    };
    let store = options.store.as_ref().ok_or("--store is needed")?;
    let mut plans = Plans::load(path)?;
    repo.sign_rollouts(store, signers, Some(&mut plans), tuf::time::now())?;
    Ok(Some(plans))
}

/// Keeps `plans` where `--plan` says, if it was given.
fn keep_plans(options: &Options, plans: Option<&Plans>) -> Result<(), String> {
    match (&options.plan, plans) {
        (Some(path), Some(plans)) => plans.save(path),
        _ => Ok(()),
    }
}

fn say_refreshed(refreshed: &Refreshed, out: &Path) {
    match refreshed {
        Refreshed::Timestamp(version) => println!("Signed timestamp {version}."),
        Refreshed::All { snapshot, timestamp } => println!(
            "Published snapshot {snapshot} and timestamp {timestamp} in {}.",
            out.display()
        ),
    }
}

/// The plan of the rollout of version `version` of bundle `id`, which
/// only the newest version of a bundle in the catalog has.
fn plan_of<'a>(plans: &'a mut Plans, id: &str, version: &str) -> Result<&'a mut Plan, String> {
    plans.releases.get_mut(&format!("{id}/{version}")).ok_or_else(|| {
        format!(
            "{id} {version} has no rollout: only the newest version of a bundle in the \
             published catalog has one"
        )
    })
}

/// `rollout --plan FILE ...`: on the update server, the waves of a
/// bundle's newest version held or freed, or held for the canaries that
/// are not green, and the rollouts signed and published again (ADRs 0533
/// and 0544).
fn rollout(
    repo: &Repo,
    options: &Options,
    signers: &Signers,
    words: &[&str],
) -> Result<(), String> {
    let path = options.plan.as_ref().ok_or("--plan is needed")?;
    let store = options.store.as_ref().ok_or("--store is needed")?;
    let out = options.out.as_ref().ok_or("--out is needed")?;
    let mut plans = Plans::load(path)?;
    let said = match words {
        ["pause", id, version, reason] => {
            plan_of(&mut plans, id, version)?.paused = Some((*reason).to_string());
            format!("{id} {version} is paused: {reason}")
        }
        ["resume", id, version] => {
            plan_of(&mut plans, id, version)?.paused = None;
            format!("{id} {version} goes on")
        }
        ["canaries"] => {
            plans.red = canaries_red(options)?;
            verdict::said(&plans.red)
        }
        ["canary", ..] => return Err(verdict::BY_HAND.into()),
        _ => {
            return Err("rollout takes pause ID VERSION REASON, resume ID VERSION or canaries \
                        --canaries FILE --reports DIR"
                .into());
        }
    };
    if repo.sign_rollouts(store, signers, Some(&mut plans), tuf::time::now())?.is_none() {
        return Err("the targets delegate no rollouts: sign the targets again".into());
    }
    let days = (expires_days("snapshot"), expires_days("timestamp"));
    let refreshed = repo.refresh(store, out, signers, days)?;
    plans.save(path)?;
    println!("{said}.");
    say_refreshed(&refreshed, out);
    for (release, plan) in &plans.releases {
        let held = plan.paused.as_ref().map_or_else(String::new, |why| format!(", paused: {why}"));
        println!("{}: wave {} open{held}", release.replace('/', " "), plan.open);
    }
    Ok(())
}

/// The bundles in `dir`, the `.dkv` files, by name: those `sign` takes
/// when none is named. None without a directory or when it cannot be read.
fn bundles_in(dir: Option<&Path>) -> Vec<PathBuf> {
    let Some(entries) = dir.and_then(|dir| std::fs::read_dir(dir).ok()) else {
        return Vec::new();
    };
    let mut bundles: Vec<PathBuf> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|e| e == dkv::EXTENSION))
        .collect();
    bundles.sort();
    bundles
}

/// `served URL`: what the catalog served at `url` says, as JSON or in
/// words.
fn served(repo: &Repo, options: &Options, url: &str) -> Result<(), String> {
    let served = repo.served(url)?;
    if options.json {
        println!("{served}");
        return Ok(());
    }
    let now = served["checked"].as_i64().unwrap_or_default();
    let bundles = served["bundles"].as_array().map_or_default(Vec::as_slice);
    let flawed = bundles.iter().filter(|b| b["flaws"].as_array().is_some_and(|f| !f.is_empty()));
    println!(
        "{}: {} bundles, {} with known flaws, signed from the first root",
        served["url"].as_str().unwrap_or(url),
        bundles.len(),
        flawed.count()
    );
    for role in served["roles"].as_array().map_or_default(Vec::as_slice) {
        let expires = role["expires"].as_i64().unwrap_or_default();
        println!(
            "{}: version {}, expires {} ({} days)",
            role["role"].as_str().unwrap_or("?"),
            role["version"],
            tuf::time::format(expires),
            (expires - now).div_euclid(86_400)
        );
    }
    for held in served["rollouts"].as_array().map_or_default(Vec::as_slice) {
        if let Some(why) = held["held"].as_str() {
            println!(
                "rollout of {} {} is held at wave {}: {why}",
                held["bundle"].as_str().unwrap_or("?"),
                held["version"].as_str().unwrap_or("?"),
                held["wave"]
            );
        }
    }
    Ok(())
}

/// `status --json`: what `status` prints, as JSON, and what `sign` would
/// change with the bundles in `--bundles`: the bundles it would add or
/// take out, the publishers it would mark verified, and the versions
/// packed again that it would keep as signed.
fn status_json(repo: &Repo, options: &Options) -> Result<(), String> {
    println!("{}", status_value(repo, options)?);
    Ok(())
}

fn status_value(repo: &Repo, options: &Options) -> Result<Value, String> {
    let mut roles = Vec::new();
    if let Some(root) = repo.root()? {
        roles.push(json!({
            "role": "root", "version": root.signed.version, "expires": root.signed.expires
        }));
    }
    let mut listed = 0;
    if let Some(targets) = repo.targets()? {
        listed = targets.signed.signed.targets.len();
        roles.push(json!({
            "role": "targets", "version": targets.signed.version, "expires": targets.signed.expires
        }));
    }
    let keys: Vec<Value> = keys::read_all(&repo.keys_dir())?
        .iter()
        .map(|key| {
            json!({
                "name": key.name,
                "id": key.key.id(),
                "holder": if key.yubikey.is_some() { "yubikey" } else { "file" },
                "serial": key.yubikey,
                "roles": key.roles,
            })
        })
        .collect();
    let bundles = bundles_in(options.bundles.as_deref());
    let changes = repo.changes(&bundles, &[], &options.verified, options.store.as_deref())?;
    let added: Vec<Value> = changes
        .added
        .iter()
        .map(|(path, info)| {
            json!({ "path": path, "id": info["custom"]["id"], "version": info["custom"]["version"] })
        })
        .collect();
    let targets = repo.targets()?.map_or_default(|t| t.signed.signed.targets);
    let verified: Vec<Value> = changes
        .verified
        .iter()
        .map(|path| {
            let custom = targets.get(path).and_then(|t| t.custom.clone()).unwrap_or_default();
            json!({ "path": path, "id": custom["id"], "version": custom["version"] })
        })
        .collect();
    let rebuilt: Vec<Value> = changes
        .rebuilt
        .iter()
        .map(|r| {
            json!({
                "path": r.path,
                "id": r.id,
                "version": r.version,
                "files": r.files.json(),
                "next": r.next,
            })
        })
        .collect();
    Ok(json!({
        "roles": roles,
        "bundles_listed": listed,
        "keys": keys,
        "unsigned": {
            "added": added,
            "removed": changes.removed,
            "verified": verified,
            "rebuilt": rebuilt,
        },
        "warn_days": WARN_DAYS,
    }))
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
        Ok(status) => status,
        Err(err) => {
            eprintln!("catalog-repo: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(what: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("catalog-repo-main-{}-{what}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn options(words: &[&str]) -> Options {
        parse(words.iter().map(|w| (*w).to_string()).collect()).unwrap()
    }

    #[test]
    fn the_catalog_follows_the_canaries_the_plan_does_not_yet() {
        let dir = scratch("canaries");
        let (list, reports, plan) =
            (dir.join("canaries.json"), dir.join("reports"), dir.join("rollout.json"));
        std::fs::create_dir(&reports).unwrap();
        let ref_box = system_release::canary::Key::from_pkcs8(
            &system_release::canary::Key::generate().unwrap(),
        )
        .unwrap();
        let mut canaries = system_release::canary::Canaries::default();
        canaries.add("ref-box", &ref_box.public_hex(), 0).unwrap();
        canaries.save(&list).unwrap();
        let (list_s, reports_s, plan_s) =
            (list.display().to_string(), reports.display().to_string(), plan.display().to_string());
        let changed = |now: &str| {
            canaries_changed(&options(&[
                "--canaries",
                &list_s,
                "--reports",
                &reports_s,
                "--plan",
                &plan_s,
                "--now",
                now,
                "canaries",
                "changed",
            ]))
        };
        // No report yet: the canary is not green, and the plan, which
        // holds nothing, is out of step.
        assert_eq!(changed("1800000000"), Ok(true));
        Plans { red: vec!["ref-box".into()], ..Plans::default() }.save(&plan).unwrap();
        assert_eq!(changed("1800000000"), Ok(false));
        // A fresh healthy report frees it, so the plan changes again.
        let mut report =
            system_release::canary::Report::new("ref-box", 1_800_000_000 - 60, "0.16.0", "beta");
        report.health = Some(system_release::canary::Health {
            passed: true,
            time: 1_800_000_000,
            detail: String::new(),
        });
        report.check =
            Some(system_release::canary::Check { failures: 0, warnings: 0, failed: Vec::new() });
        std::fs::write(reports.join("ref-box.json"), ref_box.seal(&report)).unwrap();
        assert_eq!(changed("1800000000"), Ok(true));
        // Silent two hours on, it is not green again, as the plan says.
        assert_eq!(changed(&(1_800_000_000 + 3 * 3600).to_string()), Ok(false));
        // The plan or the list missing is an error, not a change.
        let missing = options(&["--canaries", &list_s, "--reports", &reports_s, "canaries"]);
        assert!(canaries_changed(&missing).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn marking_a_canary_by_rollout_is_refused_with_the_new_command() {
        let dir = scratch("by-hand");
        let words = [
            "--repo", "r", "--store", "s", "--out", "o", "--plan", "p", "rollout", "canary", "x",
            "red",
        ];
        let options = options(&words);
        let repo = Repo::new(&dir);
        let err = rollout(&repo, &options, &Signers::default(), &["canary", "x", "red"]);
        let err = err.unwrap_err();
        assert!(err.contains("./ob release canary NAME red REASON"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A catalog in `dir/catalog` whose root and targets a key file signs,
    /// with `bundles` signed: its repository and the key file.
    fn catalog(dir: &Path, bundles: &[PathBuf]) -> (Repo, PathBuf) {
        let repo = Repo::new(&dir.join("catalog"));
        let secret = dir.join("offline.key");
        let roles = catalog_repo::ROLES.iter().map(|r| (*r).to_string()).collect();
        keys::new_file_key("offline", roles, &secret).unwrap().write(&repo.keys_dir()).unwrap();
        let signers = Signers::new(None, std::slice::from_ref(&secret)).unwrap();
        repo.sign_root(&signers, 365).unwrap();
        let changes = repo.changes(bundles, &[], &[], None).unwrap();
        repo.sign_targets(&changes, bundles, &dir.join("store"), &signers, 182).unwrap();
        (repo, secret)
    }

    /// A bundle of `id` and `version` in `dir`, its program `content`.
    fn bundle(dir: &Path, id: &str, version: &str, content: &[u8]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let manifest = format!(r#"{{"id": "{id}", "version": "{version}", "name": "Test"}}"#);
        let path = dir.join(format!("{id}-{version}.dkv"));
        let mut zip = dkv::zip::Writer::new(std::fs::File::create(&path).unwrap());
        for (name, data) in [(dkv::MANIFEST, manifest.as_bytes()), ("bin/tool", content)] {
            zip.add(&dkv::zip::Input { path: name, executable: false, data }).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    /// `catalog-repo` run on `repo` with `words`, signing through `secret`,
    /// and how many times the keys were looked for.
    fn sign_with(repo: &Repo, secret: &Path, words: &[&str]) -> (Result<(), String>, usize) {
        let options = options(words);
        let asked = std::cell::Cell::new(0);
        let signers = || {
            asked.set(asked.get() + 1);
            Signers::new(None, &[secret.to_path_buf()])
        };
        let named: Vec<&str> = options.rest.iter().skip(1).map(String::as_str).collect();
        let result = match options.rest.first().map(String::as_str) {
            Some("sign") => sign(repo, &options, &named, signers),
            Some("withdraw") => withdraw(repo, &options, signers),
            _ => Err("no such command here".into()),
        };
        (result, asked.get())
    }

    fn targets_bytes(repo: &Repo) -> Vec<u8> {
        repo.targets().unwrap().unwrap().bytes
    }

    #[test]
    fn a_preview_signs_nothing() {
        let dir = scratch("preview");
        let (repo, secret) =
            catalog(&dir, &[bundle(&dir.join("old"), "org.dukovo.rg", "1", b"rg")]);
        let bundles = dir.join("bundles");
        bundle(&bundles, "org.dukovo.bat", "0.26.1", b"bat");
        let (store, plan) = (dir.join("store"), dir.join("out/catalog/plan.json"));
        let (store_s, bundles_s, plan_s) = (
            store.display().to_string(),
            bundles.display().to_string(),
            plan.display().to_string(),
        );
        let before = targets_bytes(&repo);
        let words = ["--store", &store_s, "--bundles", &bundles_s, "--preview", &plan_s, "sign"];
        let (result, asked) = sign_with(&repo, &secret, &words);
        assert_eq!(result, Ok(()));
        assert_eq!(asked, 0, "a preview looks for no key");
        assert_eq!(targets_bytes(&repo), before);
        assert!(!store.join("bundles/org.dukovo.bat-0.26.1.dkv").exists());
        let written: Value = serde_json::from_slice(&std::fs::read(&plan).unwrap()).unwrap();
        for field in ["format", "action", "targets", "signers", "added", "removed", "verified"] {
            assert!(written.get(field).is_some(), "the plan has {field}");
        }
        for field in ["withdrawn", "rebuilt"] {
            assert!(written.get(field).is_some(), "the plan has {field}");
        }
        assert_eq!(written["added"][0]["path"], "bundles/org.dukovo.bat-0.26.1.dkv");
        assert!(is_digest(written["digest"].as_str().unwrap()));
        // Signing it as previewed asks for no answer and signs it.
        let digest = written["digest"].as_str().unwrap().to_string();
        let words = ["--store", &store_s, "--bundles", &bundles_s, "--expect", &digest, "sign"];
        let (result, asked) = sign_with(&repo, &secret, &words);
        assert_eq!(result, Ok(()));
        assert_eq!(asked, 1);
        assert_eq!(repo.targets().unwrap().unwrap().signed.version, 2);
        assert!(store.join("bundles/org.dukovo.bat-0.26.1.dkv").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_plan_that_moved_is_refused_before_signing() {
        let dir = scratch("moved");
        let (repo, secret) =
            catalog(&dir, &[bundle(&dir.join("old"), "org.dukovo.rg", "1", b"rg")]);
        let bundles = dir.join("bundles");
        bundle(&bundles, "org.dukovo.bat", "0.26.1", b"bat");
        let (store, plan) = (dir.join("store"), dir.join("plan.json"));
        let (store_s, bundles_s, plan_s) = (
            store.display().to_string(),
            bundles.display().to_string(),
            plan.display().to_string(),
        );
        let words = ["--store", &store_s, "--bundles", &bundles_s, "--preview", &plan_s, "sign"];
        assert_eq!(sign_with(&repo, &secret, &words).0, Ok(()));
        let written: Value = serde_json::from_slice(&std::fs::read(&plan).unwrap()).unwrap();
        let digest = written["digest"].as_str().unwrap().to_string();
        // A bundle more in the folder after the preview: what would be
        // signed now is not what was read.
        bundle(&bundles, "org.dukovo.jq", "1.7.1", b"jq");
        let before = targets_bytes(&repo);
        let words = ["--store", &store_s, "--bundles", &bundles_s, "--expect", &digest, "sign"];
        let (result, asked) = sign_with(&repo, &secret, &words);
        let err = result.unwrap_err();
        assert!(err.contains("what would be signed now is not what was previewed"), "{err}");
        assert!(err.ends_with("prepare again"), "{err}");
        assert_eq!(asked, 0, "no key is looked for");
        assert_eq!(targets_bytes(&repo), before);
        // Something else than a digest is refused as such.
        let words = ["--store", &store_s, "--expect", "ABC", "--renew", "sign"];
        assert!(sign_with(&repo, &secret, &words).0.unwrap_err().contains("64 hex digits"));
        // And so is more than one way to settle it.
        let words = ["--store", &store_s, "--yes", "--expect", &digest, "sign"];
        assert!(sign_with(&repo, &secret, &words).0.unwrap_err().contains("give one of"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_withdrawal_is_previewed_and_held_to_its_digest() {
        let dir = scratch("withdraw-plan");
        let (repo, secret) =
            catalog(&dir, &[bundle(&dir.join("old"), "org.dukovo.rg", "1", b"rg")]);
        let (store_s, plan) = (dir.join("store").display().to_string(), dir.join("plan.json"));
        let plan_s = plan.display().to_string();
        let withdraw = |reason: &str, settle: [&str; 2], now: &str| {
            let words = [
                "--store",
                &store_s,
                "--id",
                "org.dukovo.rg",
                "--version",
                "1",
                "--kind",
                "broken",
                "--reason",
                reason,
                "--now",
                now,
                settle[0],
                settle[1],
                "withdraw",
            ];
            sign_with(&repo, &secret, &words)
        };
        assert_eq!(withdraw("it crashes", ["--preview", &plan_s], "10").0, Ok(()));
        let written: Value = serde_json::from_slice(&std::fs::read(&plan).unwrap()).unwrap();
        assert_eq!(written["action"], "withdraw");
        assert_eq!(written["withdrawn"][0]["reason"], "it crashes");
        assert_eq!(repo.targets().unwrap().unwrap().signed.version, 1);
        let digest = written["digest"].as_str().unwrap();
        let (result, asked) = withdraw("it crashes at start", ["--expect", digest], "20");
        assert!(result.unwrap_err().contains("prepare again"));
        assert_eq!(asked, 0);
        // A minute later, the same withdrawal signs.
        assert_eq!(withdraw("it crashes", ["--expect", digest], "70"), (Ok(()), 1));
        assert_eq!(repo.targets().unwrap().unwrap().signed.version, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_renewal_signs_no_bundle_and_refuses_one_named() {
        let dir = scratch("renew");
        let (repo, secret) =
            catalog(&dir, &[bundle(&dir.join("old"), "org.dukovo.rg", "1", b"rg")]);
        let bundles = dir.join("bundles");
        let bat = bundle(&bundles, "org.dukovo.bat", "0.26.1", b"bat");
        let (store_s, bundles_s) =
            (dir.join("store").display().to_string(), bundles.display().to_string());
        let bat_s = bat.display().to_string();
        let named = ["--store", &store_s, "--renew", "--yes", "sign", &bat_s];
        let (result, asked) = sign_with(&repo, &secret, &named);
        assert!(result.unwrap_err().contains("name no bundle"));
        assert_eq!(asked, 0);
        let removal = ["--store", &store_s, "--renew", "--remove", "x", "--yes", "sign"];
        assert!(sign_with(&repo, &secret, &removal).0.unwrap_err().contains("take none out"));
        // With bundles in --bundles, a renewal reads none of them.
        let words = ["--store", &store_s, "--bundles", &bundles_s, "--renew", "--yes", "sign"];
        assert_eq!(sign_with(&repo, &secret, &words), (Ok(()), 1));
        let targets = repo.targets().unwrap().unwrap().signed;
        assert_eq!(targets.version, 2);
        assert_eq!(targets.signed.targets.len(), 1, "the targets are renewed as they were");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn status_lists_a_repacked_version_and_pending_marks() {
        let dir = scratch("status");
        let (repo, _) = catalog(&dir, &[bundle(&dir.join("old"), "org.dukovo.rg", "1", b"rg")]);
        let bundles = dir.join("bundles");
        bundle(&bundles, "org.dukovo.rg", "1", b"rg, linked again");
        bundle(&bundles, "org.dukovo.bat", "0.26.1", b"bat");
        let bundles_s = bundles.display().to_string();
        let words = ["--bundles", &bundles_s, "--verified", "org.dukovo.rg", "--json", "status"];
        let status = status_value(&repo, &options(&words)).unwrap();
        let unsigned = &status["unsigned"];
        let rg =
            json!([{"path": "bundles/org.dukovo.rg-1.dkv", "id": "org.dukovo.rg", "version": "1"}]);
        let kept = json!([{
            "path": "bundles/org.dukovo.rg-1.dkv", "id": "org.dukovo.rg", "version": "1",
            "files": null, "next": "1_2"
        }]);
        assert_eq!(unsigned["rebuilt"], kept, "without --store, the files are not compared");
        assert_eq!(unsigned["verified"], rg);
        let bat = json!([{
            "path": "bundles/org.dukovo.bat-0.26.1.dkv", "id": "org.dukovo.bat", "version": "0.26.1"
        }]);
        assert_eq!(unsigned["added"], bat);
        assert_eq!(unsigned["removed"], json!([]));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
