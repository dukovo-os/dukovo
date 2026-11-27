// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The system-release command (system-release(8)): `ob` runs it on the
//! build machine to make keys and sign releases and the update
//! repository that offers them, and Dukovo has it as one of the native
//! tools, to check an install stick's release or an update file.

use std::path::{Path, PathBuf};

use crate::advisory;
use crate::canary::{self, Canaries, Colour, Standing};
use crate::ceremony;
use crate::list::{About, Release};
use crate::repo::{self, Keys};
use crate::rollout::Plans;
use crate::{updates, verdict};

/// Where a Dukovo system keeps the root of the releases it trusts.
pub const TRUSTED_ROOT: &str = "/system/resources/updates/root.json";

const USAGE: &str = "\
usage: system-release keys [--rotate ROLE | --online KEY] DIR
       system-release online-key --out KEY SECRET
       system-release release-key --yubikey-helper FILE [--serial NUMBER] KEYS
       system-release first-root --yubikey-helper FILE --out METADATA KEYS
       system-release make --tree DIR --keys DIR --out DIR --version VERSION
                           --commit COMMIT --toolchain TOOLCHAIN --built SECONDS
                           [--updates-from VERSION]
       system-release list --tree DIR --version VERSION --commit COMMIT
                           --toolchain TOOLCHAIN --built SECONDS
                           [--updates-from VERSION] [--out FILE]
       system-release add --repo DIR --keys DIR RELEASE
       system-release fetch --root FILE --out DIR URL
       system-release medium --root FILE --tree DIR --out DIR URL LIST
       system-release sums --root FILE --yubikey-helper FILE [--serial NUMBER]
                           --version VERSION --out DIR [--record FILE] [--yes] IMAGE...
       system-release offer --repo DIR SIGNER --tree DIR [--advisories SOURCE]
                            [--record FILE] [--yes] LIST CHANNEL
       system-release firmware --repo DIR --keys DIR --version VERSION
                               --microcode FILE --license LICENSE
       system-release pull --repo DIR SIGNER --version VERSION --reason TEXT
                           --advice rollback|stay
       system-release channel --repo DIR SIGNER [--advisories SOURCE] CHANNEL VERSION
       system-release channel --repo DIR SIGNER [--advisories SOURCE]
                              --firmware VERSION|none CHANNEL
       system-release advisories --repo DIR SIGNER SOURCE
       system-release advisories --check SOURCE
       system-release refresh --repo DIR (--keys DIR | --online-key SECRET [--plan FILE])
       system-release publish --repo DIR --online-key SECRET [--plan FILE] INCOMING
       system-release rollout --repo DIR --online-key SECRET --plan FILE
                              (pause VERSION REASON | resume VERSION
                               | canaries --canaries FILE --reports DIR)
       system-release canary-receive --socket PATH --canaries FILE --reports DIR
       system-release canary --canaries FILE
                             (add NAME KEY | remove NAME | NAME red REASON
                              | NAME green REASON | NAME auto)
       system-release canaries show --canaries FILE --reports DIR
                                    [--repo DIR --plan FILE] [--json]
       system-release canaries changed --canaries FILE --reports DIR --repo DIR --plan FILE
       system-release pack --repo DIR [--root FILE] --version VERSION --out FILE
       system-release check [--root FILE] [--version VERSION] REPOSITORY|FILE
       system-release served [--json] [--root FILE] URL
       system-release rescue --out ESP [--root FILE] [--version VERSION] REPOSITORY
SIGNER is --keys DIR, or --yubikey-helper FILE [--serial NUMBER] to sign on a
YubiKey, with the release key the repository's newest root names.";

/// How many damaged files `check` names before it counts the rest.
const SHOWN_PROBLEMS: usize = 20;

/// Runs the command with `args`, those after its name, and gives its exit
/// status: 0 when it did what it was asked, 1 when it failed, 2 for a
/// usage error.
#[must_use]
pub fn run(args: &[String]) -> i32 {
    let Some((command, rest)) = args.split_first() else {
        eprintln!("{USAGE}");
        return 2;
    };
    let options = match Options::parse(rest) {
        Ok(options) => options,
        Err(why) => {
            eprintln!("system-release: {why}\n{USAGE}");
            return 2;
        }
    };
    let done = match command.as_str() {
        "keys" => keys(&options),
        "online-key" => online_key(&options),
        "release-key" => release_key(&options),
        "first-root" => first_root(&options),
        "make" => make(&options),
        "list" => list(&options),
        "add" => add(&options),
        "fetch" => fetch(&options),
        "medium" => medium(&options),
        "sums" => sums(&options),
        "offer" => offer(&options),
        "firmware" => firmware(&options),
        "pull" => pull(&options),
        "channel" => channel(&options),
        "advisories" => advisories(&options),
        "refresh" => refresh(&options),
        "publish" => publish(&options),
        "rollout" => rollout(&options),
        "canary-receive" => canary_receive(&options),
        "canary" => canary(&options),
        "canaries" => match canaries(&options) {
            Ok(status) => return status,
            Err(why) => Err(why),
        },
        "pack" => pack(&options),
        "check" => check(&options),
        "served" => served(&options),
        "rescue" => rescue(&options),
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            return 0;
        }
        other => {
            eprintln!("system-release: no command `{other}`\n{USAGE}");
            return 2;
        }
    };
    match done {
        Ok(()) => 0,
        Err(why) => {
            eprintln!("system-release: {why}");
            1
        }
    }
}

/// The options and operands, each command taking those it needs.
#[derive(Default)]
struct Options {
    tree: Option<PathBuf>,
    keys: Option<PathBuf>,
    repo: Option<PathBuf>,
    rotate: Option<String>,
    online: Option<PathBuf>,
    online_key: Option<PathBuf>,
    out: Option<PathBuf>,
    root: Option<PathBuf>,
    version: Option<String>,
    commit: Option<String>,
    toolchain: Option<String>,
    built: Option<i64>,
    updates_from: Option<String>,
    microcode: Option<PathBuf>,
    license: Option<String>,
    firmware: Option<String>,
    advisories: Option<PathBuf>,
    reason: Option<String>,
    advice: Option<String>,
    plan: Option<PathBuf>,
    canaries: Option<PathBuf>,
    reports: Option<PathBuf>,
    socket: Option<PathBuf>,
    yubikey_helper: Option<PathBuf>,
    serial: Option<u64>,
    /// For `offer` and `sums`: where to write the Rekor entry of the
    /// signature.
    record: Option<PathBuf>,
    /// For `offer` and `sums`: sign without asking for the version to be
    /// typed.
    yes: bool,
    /// For tests: the time the canaries are judged at, in place of now.
    now: Option<i64>,
    json: bool,
    /// For `advisories`: only check them.
    check: bool,
    operands: Vec<String>,
}

impl Options {
    fn parse(args: &[String]) -> Result<Options, String> {
        let mut options = Options::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = || args.next().cloned().ok_or(format!("{arg} needs a value"));
            match arg.as_str() {
                "--tree" => options.tree = Some(value()?.into()),
                "--keys" => options.keys = Some(value()?.into()),
                "--repo" => options.repo = Some(value()?.into()),
                "--rotate" => options.rotate = Some(value()?),
                "--online" => options.online = Some(value()?.into()),
                "--online-key" => options.online_key = Some(value()?.into()),
                "--out" => options.out = Some(value()?.into()),
                "--root" => options.root = Some(value()?.into()),
                "--version" => options.version = Some(value()?),
                "--commit" => options.commit = Some(value()?),
                "--toolchain" => options.toolchain = Some(value()?),
                "--updates-from" => options.updates_from = Some(value()?),
                "--microcode" => options.microcode = Some(value()?.into()),
                "--license" => options.license = Some(value()?),
                "--firmware" => options.firmware = Some(value()?),
                "--advisories" => options.advisories = Some(value()?.into()),
                "--reason" => options.reason = Some(value()?),
                "--advice" => options.advice = Some(value()?),
                "--plan" => options.plan = Some(value()?.into()),
                "--canaries" => options.canaries = Some(value()?.into()),
                "--reports" => options.reports = Some(value()?.into()),
                "--socket" => options.socket = Some(value()?.into()),
                "--yubikey-helper" => options.yubikey_helper = Some(value()?.into()),
                "--record" => options.record = Some(value()?.into()),
                "--yes" => options.yes = true,
                "--serial" => {
                    let text = value()?;
                    let serial = text.parse().map_err(|_| {
                        format!("--serial takes a YubiKey's serial number, not {text}")
                    })?;
                    options.serial = Some(serial);
                }
                "--now" => {
                    let text = value()?;
                    let seconds = text
                        .parse()
                        .map_err(|_| format!("--now takes seconds since 1970, not {text}"))?;
                    options.now = Some(seconds);
                }
                "--json" => options.json = true,
                "--check" => options.check = true,
                "--built" => {
                    let text = value()?;
                    let seconds = text
                        .parse()
                        .map_err(|_| format!("--built takes seconds since 1970, not {text}"))?;
                    options.built = Some(seconds);
                }
                option if option.starts_with('-') => return Err(format!("no option {option}")),
                operand => options.operands.push(operand.to_string()),
            }
        }
        Ok(options)
    }
}

/// `n` of `thing`, as `1 file` or `2 files`.
fn count(n: usize, thing: &str) -> String {
    if n == 1 { format!("1 {thing}") } else { format!("{n} {thing}s") }
}

/// Gives `option`'s value, or says it is needed.
fn needed<'a, T>(option: &'a Option<T>, name: &str) -> Result<&'a T, String> {
    option.as_ref().ok_or(format!("{name} is needed"))
}

/// `keys DIR`: new keys for the four roles, and the root they make; with
/// `--rotate ROLE`, a new key for that role, and the next root; with
/// `--online KEY`, the next root names the update server's key for
/// snapshot and timestamp.
fn keys(options: &Options) -> Result<(), String> {
    let [dir] = options.operands.as_slice() else {
        return Err("keys takes the folder to make them in".into());
    };
    let now = tuf::time::now();
    if let Some(public) = &options.online {
        let text = std::fs::read(public).map_err(|err| format!("{}: {err}", public.display()))?;
        let key: tuf::keys::Key = serde_json::from_slice(&text)
            .map_err(|err| format!("{}: not a public key: {err}", public.display()))?;
        let keys = Keys::set_online(Path::new(dir), &key, now)?;
        println!(
            "system-release: root {} names the update server's key {} for snapshot and \
             timestamp, beside the keys in {dir}",
            keys.roots().len(),
            &key.id()[..16]
        );
        return Ok(());
    }
    if let Some(role) = &options.rotate {
        let keys = Keys::rotate(Path::new(dir), role, now)?;
        println!(
            "system-release: a new {role} key in {dir}, named by root {}, which root {} signed",
            keys.roots().len(),
            keys.roots().len() - 1
        );
        return Ok(());
    }
    Keys::create(Path::new(dir), now)?;
    println!(
        "system-release: made keys for root, targets, snapshot and timestamp in {dir}, and root.json"
    );
    Ok(())
}

/// `online-key --out KEY SECRET`: the update server's online key, its
/// secret half in a new file `SECRET`, readable by its owner alone, and
/// its public half in `KEY`, for the next root to name (ADR 0467).
fn online_key(options: &Options) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    use tuf::repo::Signer as _;
    let [secret_path] = options.operands.as_slice() else {
        return Err("online-key takes the file to keep its secret half in".into());
    };
    let out = needed(&options.out, "--out")?;
    let (key, secret) = tuf::repo::Ed25519::generate()?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(secret_path)
        .map_err(|err| format!("{secret_path}: {err}"))?;
    file.write_all(&secret).map_err(|err| format!("{secret_path}: {err}"))?;
    let mut public = serde_json::to_vec_pretty(key.key()).map_err(|err| err.to_string())?;
    public.push(b'\n');
    std::fs::write(out, public).map_err(|err| format!("{}: {err}", out.display()))?;
    println!(
        "system-release: the online key {}: its secret half in {secret_path}, its public half in {}",
        &key.key().id()[..16],
        out.display()
    );
    Ok(())
}

/// `release-key --yubikey-helper FILE [--serial NUMBER] KEYS`: keeps the
/// public half of a YubiKey's release key, in its slot 95, in the folder
/// `KEYS` (ADR 0538).
fn release_key(options: &Options) -> Result<(), String> {
    let [dir] = options.operands.as_slice() else {
        return Err("release-key takes the folder of the release keys' public halves".into());
    };
    let helper = needed(&options.yubikey_helper, "--yubikey-helper")?;
    let path = ceremony::add_yubikey(Path::new(dir), helper, options.serial)?;
    println!(
        "system-release: wrote {} and the key in PEM, {}",
        path.display(),
        path.with_extension("pem").display()
    );
    Ok(())
}

/// `first-root --yubikey-helper FILE --out METADATA KEYS`: the key
/// ceremony's root, signed on both YubiKeys whose keys are in `KEYS`,
/// written to `METADATA/1.root.json`, with what identifies it.
fn first_root(options: &Options) -> Result<(), String> {
    let [dir] = options.operands.as_slice() else {
        return Err("first-root takes the folder of the release keys' public halves".into());
    };
    let helper = needed(&options.yubikey_helper, "--yubikey-helper")?;
    let metadata = needed(&options.out, "--out")?;
    let root = ceremony::sign_first_root(Path::new(dir), metadata, helper, tuf::time::now())?;
    let parsed = tuf::metadata::Signed::<tuf::metadata::Root>::parse(&root)
        .map_err(|err| err.to_string())?;
    let sha256 = ring::digest::digest(&ring::digest::SHA256, &root);
    println!(
        "system-release: signed the first root, {}, on both YubiKeys; it expires {}",
        metadata.join(ceremony::FIRST_ROOT).display(),
        tuf::time::format(parsed.expires)
    );
    println!("SHA-256 {}", crate::list::hex(sha256.as_ref()));
    Ok(())
}

/// The keys `--keys` or `--online-key` name for the repository `dir`.
fn signing_keys(options: &Options, dir: &Path) -> Result<Keys, String> {
    match (&options.keys, &options.online_key) {
        (Some(keys), None) => Keys::load(keys),
        (None, Some(secret)) => {
            let secret =
                std::fs::read(secret).map_err(|err| format!("{}: {err}", secret.display()))?;
            updates::server_keys(dir, &secret)
        }
        _ => Err("name the keys: --keys or --online-key".into()),
    }
}

/// The keys that sign the targets of the repository at `dir`: those in
/// the folder `--keys` names, or the release key of the YubiKey plugged
/// in, or of `--serial`, through `--yubikey-helper`, which the
/// repository's newest root must name for targets (ADR 0538).
fn targets_keys(options: &Options, dir: &Path) -> Result<Keys, String> {
    match (&options.keys, &options.yubikey_helper) {
        (Some(keys), None) => Keys::load(keys),
        (None, Some(helper)) => {
            let yubikey = crate::yubikey::release_key(helper, options.serial)?;
            Keys::hardware(repo::roots_in(dir)?, &yubikey, &["targets"])
        }
        _ => Err("name the keys: --keys or --yubikey-helper".into()),
    }
}

/// The words after a change to the repository whose keys do not sign
/// for snapshot and timestamp.
fn signed_here(keys: &Keys) -> &'static str {
    if repo::ONLINE.iter().all(|role| keys.has(role)) {
        ""
    } else {
        "; the update server signs the snapshot and timestamp once it is deployed"
    }
}

/// `make`: the release of a staged tree, signed, laid out in a new folder.
fn make(options: &Options) -> Result<(), String> {
    let tree = needed(&options.tree, "--tree")?;
    let out = needed(&options.out, "--out")?;
    let keys = Keys::load(needed(&options.keys, "--keys")?)?;
    let version = needed(&options.version, "--version")?.clone();
    let about = About {
        updates_from: options.updates_from.clone().unwrap_or_else(|| version.clone()),
        version,
        commit: needed(&options.commit, "--commit")?.clone(),
        toolchain: needed(&options.toolchain, "--toolchain")?.clone(),
        built: *needed(&options.built, "--built")?,
    };
    let release = Release::of_tree(tree, about)?;
    let published = repo::publish(&keys, tree, &release, out)?;
    println!(
        "system-release: release {} ({}) signed, {} files of {} MiB, in {}",
        published.release.version,
        &published.name[..16],
        published.release.files.iter().filter(|e| e.file().is_some()).count(),
        published.release.bytes() >> 20,
        out.display()
    );
    Ok(())
}

/// `list`: the release list of a staged tree, unsigned: the bytes `make`
/// signs as a TUF target, written to `--out`, or to the standard output.
/// It needs no keys, so that builds on different machines, whose keys
/// differ, can compare what they would sign (ADR 0535).
fn list(options: &Options) -> Result<(), String> {
    let tree = needed(&options.tree, "--tree")?;
    let version = needed(&options.version, "--version")?.clone();
    let about = About {
        updates_from: options.updates_from.clone().unwrap_or_else(|| version.clone()),
        version,
        commit: needed(&options.commit, "--commit")?.clone(),
        toolchain: needed(&options.toolchain, "--toolchain")?.clone(),
        built: *needed(&options.built, "--built")?,
    };
    let bytes = Release::of_tree(tree, about)?.to_bytes();
    match &options.out {
        Some(out) => std::fs::write(out, &bytes).map_err(|err| format!("{}: {err}", out.display())),
        None => {
            use std::io::Write as _;
            std::io::stdout().write_all(&bytes).map_err(|err| format!("the standard output: {err}"))
        }
    }
}

/// `add --repo DIR RELEASE`: the release that `make` signed, in the update
/// repository.
fn add(options: &Options) -> Result<(), String> {
    let [release] = options.operands.as_slice() else {
        return Err("add takes the folder of a release that make signed".into());
    };
    let dir = needed(&options.repo, "--repo")?;
    let keys = Keys::load(needed(&options.keys, "--keys")?)?;
    let added = updates::add(dir, &keys, Path::new(release), tuf::time::now())?;
    let version = &added.published.release.version;
    if added.already {
        println!("system-release: {} offers Dukovo {version} already", dir.display());
    } else {
        println!(
            "system-release: {} offers Dukovo {version}, with {} its store did not hold{}",
            dir.display(),
            count(added.new_files, "file"),
            signed_here(&keys)
        );
    }
    Ok(())
}

/// `fetch --root FILE --out DIR URL`: what the update server at `URL`
/// serves, read from the first root `FILE`, as a new repository in `DIR`
/// for `offer` to change (ADR 0778).
fn fetch(options: &Options) -> Result<(), String> {
    let [url] = options.operands.as_slice() else {
        return Err("fetch takes the address of an update server's repository".into());
    };
    let root_path = needed(&options.root, "--root")?;
    let out = needed(&options.out, "--out")?;
    let root = std::fs::read(root_path).map_err(|err| format!("{}: {err}", root_path.display()))?;
    let url = if url.ends_with('/') { url.clone() } else { format!("{url}/") };
    let base = tuf::Url::parse(&url).map_err(|err| format!("{url}: {err}"))?;
    let fetched = repo::fetch(&base, &root, out)?;
    if fetched.targets == 0 {
        println!(
            "system-release: {url} serves no releases yet; {} holds the first root",
            out.display()
        );
    } else {
        println!(
            "system-release: fetched root version {} and targets version {}, with {}, from {url} \
             into {}",
            fetched.root,
            fetched.targets,
            count(fetched.listed, "target"),
            out.display()
        );
    }
    Ok(())
}

/// `medium --root FILE --tree DIR --out DIR URL LIST`: the release whose
/// list `./ob release build` wrote in `LIST`, as the update server at
/// `URL` serves it, read from the first root `FILE`, laid out in the new
/// folder `DIR` for an install stick or ISO, with its files from the tree
/// `--tree` (ADR 0779).
fn medium(options: &Options) -> Result<(), String> {
    let [url, list] = options.operands.as_slice() else {
        return Err("medium takes the update server's address and the release list".into());
    };
    let root_path = needed(&options.root, "--root")?;
    let tree = needed(&options.tree, "--tree")?;
    let out = needed(&options.out, "--out")?;
    let root = std::fs::read(root_path).map_err(|err| format!("{}: {err}", root_path.display()))?;
    let list = std::fs::read(list).map_err(|err| format!("{list}: {err}"))?;
    let url = if url.ends_with('/') { url.clone() } else { format!("{url}/") };
    let base = tuf::Url::parse(&url).map_err(|err| format!("{url}: {err}"))?;
    let medium = crate::medium::write(&base, &root, &list, tree, out)?;
    println!(
        "system-release: Dukovo {} ({}) as {url} serves it, {} of {} MiB, in {}: root version \
         {}, timestamp {}, snapshot {}, targets {}",
        medium.version,
        &medium.name[..16],
        count(medium.files, "file"),
        medium.bytes >> 20,
        out.display(),
        medium.root,
        medium.timestamp,
        medium.snapshot,
        medium.targets
    );
    Ok(())
}

/// `sums --root FILE --yubikey-helper FILE --version VERSION --out DIR
/// IMAGE...`: the checksums of the install images of Dukovo `VERSION`,
/// signed on a YubiKey with the release key the root `FILE` names for
/// targets, after they are shown and the version typed, into `DIR`. With
/// `--record FILE`, the Rekor entry of the signature is written to `FILE`
/// (ADR 0779).
fn sums(options: &Options) -> Result<(), String> {
    use tuf::repo::Signer as _;
    if options.operands.is_empty() {
        return Err("sums takes the images to sign the checksums of".into());
    }
    let root_path = needed(&options.root, "--root")?;
    let helper = needed(&options.yubikey_helper, "--yubikey-helper")?;
    let version = needed(&options.version, "--version")?;
    let out = needed(&options.out, "--out")?;
    let root = std::fs::read(root_path).map_err(|err| format!("{}: {err}", root_path.display()))?;
    let yubikey = crate::yubikey::release_key(helper, options.serial)?;
    let images: Vec<PathBuf> = options.operands.iter().map(PathBuf::from).collect();
    let (named, text) = crate::sums::sums(&images)?;
    let signature = crate::sums::sign(&text, &root, &yubikey, || {
        print!("{}", sums_lines(version, &named, &yubikey));
        if options.yes { Ok(()) } else { typed(version) }
    })?;
    let [sums_file, signature_file] = crate::sums::write(out, &text, &signature, yubikey.serial())?;
    println!(
        "system-release: signed the checksums of {} on YubiKey {}: {} and {}",
        count(named.len(), "image"),
        yubikey.serial(),
        sums_file.display(),
        signature_file.display()
    );
    if let Some(record) = &options.record {
        let entry = crate::rekor::entry_for(&text, &signature, yubikey.key())?;
        std::fs::write(record, entry).map_err(|err| format!("{}: {err}", record.display()))?;
        println!("system-release: the Rekor entry of the signature is in {}", record.display());
    }
    Ok(())
}

/// What `sums` is about to sign, in lines for the person who signs it.
fn sums_lines(
    version: &str,
    images: &[crate::sums::Image],
    yubikey: &crate::yubikey::YubiKey,
) -> String {
    use std::fmt::Write as _;
    use tuf::repo::Signer as _;
    let mut text = format!(
        "To be signed on YubiKey {}, with release key {}:\n  the checksums of the install \
         images of Dukovo {version}\n",
        yubikey.serial(),
        &yubikey.key().id()[..16]
    );
    for image in images {
        let mib = image.bytes.div_ceil(1 << 20);
        let _ = writeln!(text, "  {}, {mib} MiB, SHA-256 {}", image.name, image.sha256);
    }
    text
}

/// `offer --repo DIR SIGNER --tree DIR LIST CHANNEL`: the release whose
/// list `./ob release build` wrote in `LIST`, of the tree `--tree`,
/// offered on `CHANNEL`, signed in one targets version after what is
/// about to be signed is shown and its version typed. With `--record
/// FILE`, the Rekor entry of the signature is written to `FILE` (ADR
/// 0778).
fn offer(options: &Options) -> Result<(), String> {
    let [list, channel] = options.operands.as_slice() else {
        return Err("offer takes the release list and the channel to offer it on".into());
    };
    let dir = needed(&options.repo, "--repo")?;
    let tree = needed(&options.tree, "--tree")?;
    let keys = targets_keys(options, dir)?;
    let key = keys.signer("targets")?.key().clone();
    if options.record.is_some() && !key.is_p256() {
        return Err(
            "only a YubiKey's signature is recorded in Rekor: sign with --yubikey-helper".into()
        );
    }
    let list = std::fs::read(list).map_err(|err| format!("{list}: {err}"))?;
    let sources = options.advisories.as_deref().map(load_advisories).transpose()?;
    let new = updates::NewRelease { tree, list: &list, channel, advisories: sources.as_deref() };
    let offered = updates::offer(dir, &keys, &new, tuf::time::now(), |proposal| {
        print!("{}", proposal_lines(proposal));
        if options.yes { Ok(()) } else { typed(&proposal.version) }
    })?;
    let proposal = &offered.proposal;
    println!(
        "system-release: signed targets version {}: the {} channel of {} offers Dukovo {}, with {} \
         its store did not hold{}",
        proposal.targets_version,
        proposal.channel,
        dir.display(),
        proposal.version,
        count(offered.new_files, "file"),
        signed_here(&keys)
    );
    if let Some(record) = &options.record {
        let entry = crate::rekor::entry(&offered.targets, &key)?;
        std::fs::write(record, entry).map_err(|err| format!("{}: {err}", record.display()))?;
        println!("system-release: the Rekor entry of the signature is in {}", record.display());
    }
    Ok(())
}

/// What `offer` is about to sign, in lines for the person who signs it.
fn proposal_lines(proposal: &updates::Proposal) -> String {
    let replaces = proposal
        .replaces
        .as_ref()
        .map_or_else(|| "which offers no release until now".into(), |v| format!("in place of {v}"));
    let advisories = proposal.advised.as_ref().map_or_else(
        || "  no security advisories are signed with it\n".to_string(),
        |advised| {
            let mut changes: Vec<String> = Vec::new();
            for (word, ids) in [
                ("new", &advised.added),
                ("changed", &advised.changed),
                ("removed", &advised.removed),
            ] {
                if !ids.is_empty() {
                    changes.push(format!("{word}: {}", ids.join(", ")));
                }
            }
            let changes = if changes.is_empty() { "as served".into() } else { changes.join("; ") };
            format!("  {}, {changes}\n", advisories_count(advised.count))
        },
    );
    format!(
        "To be signed:\n  Dukovo {}, release list SHA-256 {}\n  built from commit {}, {} of {} \
         MiB\n  on the {} channel, {replaces}\n{advisories}  as targets version {}, which \
         expires {}\n",
        proposal.version,
        proposal.name,
        proposal.commit,
        count(proposal.files, "file"),
        proposal.bytes >> 20,
        proposal.channel,
        proposal.targets_version,
        proposal.expires
    )
}

/// Asks for `version` to be typed, and refuses anything else: the touch
/// that signs follows at once.
fn typed(version: &str) -> Result<(), String> {
    use std::io::Write as _;
    print!("Type {version} to sign it, or anything else to stop: ");
    std::io::stdout().flush().map_err(|err| format!("the standard output: {err}"))?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|err| format!("the standard input: {err}"))?;
    if line.trim() == version {
        Ok(())
    } else {
        Err(format!("nothing was signed: {version} was not typed"))
    }
}

/// `firmware --repo DIR --version VERSION --microcode FILE --license
/// LICENSE`: a firmware set of the CPU microcode `FILE`, in the update
/// repository (ADR 0468).
fn firmware(options: &Options) -> Result<(), String> {
    let dir = needed(&options.repo, "--repo")?;
    let keys = Keys::load(needed(&options.keys, "--keys")?)?;
    let version = needed(&options.version, "--version")?;
    let file = updates::FirmwareFile {
        path: needed(&options.microcode, "--microcode")?,
        load: crate::firmware::Load::Microcode,
        license: needed(&options.license, "--license")?,
    };
    let added = updates::add_firmware(dir, &keys, version, &[file], tuf::time::now())?;
    if added.already {
        println!("system-release: {} offers firmware {version} already", dir.display());
    } else {
        println!(
            "system-release: {} offers firmware {version}, {} of {} KiB, with {} its store did \
             not hold{}",
            dir.display(),
            count(added.set.files.len(), "file"),
            added.set.bytes().div_ceil(1024),
            count(added.new_files, "file"),
            signed_here(&keys)
        );
    }
    Ok(())
}

/// `pull --repo DIR --version VERSION --reason TEXT --advice
/// rollback|stay`: the release is withdrawn (ADR 0469).
fn pull(options: &Options) -> Result<(), String> {
    let dir = needed(&options.repo, "--repo")?;
    let keys = targets_keys(options, dir)?;
    let version = needed(&options.version, "--version")?;
    let reason = needed(&options.reason, "--reason")?;
    let advice = match needed(&options.advice, "--advice")?.as_str() {
        "rollback" => crate::rollout::Advice::Rollback,
        "stay" => crate::rollout::Advice::Stay,
        other => return Err(format!("--advice {other}: say rollback or stay")),
    };
    updates::withdraw(dir, &keys, version, reason, advice, tuf::time::now())?;
    println!(
        "system-release: {} withdraws Dukovo {version}: {reason}{}",
        dir.display(),
        signed_here(&keys)
    );
    Ok(())
}

/// `channel --repo DIR CHANNEL VERSION`: the channel offers the release;
/// with `--firmware VERSION|none CHANNEL`, the firmware set, or none. With
/// `--advisories SOURCE`, the security advisories there are signed in the
/// same targets version (ADR 0534).
fn channel(options: &Options) -> Result<(), String> {
    let dir = needed(&options.repo, "--repo")?;
    let keys = targets_keys(options, dir)?;
    let sources = options.advisories.as_deref().map(load_advisories).transpose()?;
    let sources = sources.as_deref();
    let (said, advised) = if let Some(firmware) = &options.firmware {
        let [channel] = options.operands.as_slice() else {
            return Err("channel --firmware takes the channel".into());
        };
        let set = (firmware != "none").then_some(firmware.as_str());
        let advised = updates::set_channel_firmware_with(
            dir,
            &keys,
            channel,
            set,
            sources,
            tuf::time::now(),
        )?;
        let offers = set.map_or_else(|| "no firmware set".into(), |set| format!("firmware {set}"));
        (format!("the {channel} channel of {} offers {offers}", dir.display()), advised)
    } else {
        let [channel, version] = options.operands.as_slice() else {
            return Err("channel takes the channel and the version it is to offer".into());
        };
        let advised =
            updates::set_channel_with(dir, &keys, channel, version, sources, tuf::time::now())?;
        (format!("the {channel} channel of {} offers Dukovo {version}", dir.display()), advised)
    };
    println!("system-release: {said}{}", signed_here(&keys));
    if let Some(advised) = advised {
        println!("system-release: {}", advised_line(&advised, dir));
    }
    Ok(())
}

/// `n` advisories, as `1 security advisory` or `2 security advisories`.
fn advisories_count(n: usize) -> String {
    if n == 1 { "1 security advisory".into() } else { format!("{n} security advisories") }
}

/// What signing the advisories did, in a line.
fn advised_line(advised: &updates::Advised, dir: &Path) -> String {
    let mut changes = Vec::new();
    for (word, ids) in
        [("new", &advised.added), ("changed", &advised.changed), ("removed", &advised.removed)]
    {
        if !ids.is_empty() {
            changes.push(format!("{} {word}: {}", ids.len(), ids.join(", ")));
        }
    }
    let offers = format!("{} offers {}", dir.display(), advisories_count(advised.count));
    if !advised.signed {
        format!("{offers} already, those of the folder, so nothing is signed again")
    } else if changes.is_empty() {
        offers
    } else {
        format!("{offers} ({})", changes.join("; "))
    }
}

/// The advisories of the folder `source`, each checked: what is wrong with
/// any is said, and nothing is signed.
fn load_advisories(source: &Path) -> Result<Vec<advisory::Source>, String> {
    advisory::load_dir(source).map_err(|problems| {
        let shown = source.display().to_string();
        for problem in &problems {
            eprintln!("system-release: {}/{problem}", shown.trim_end_matches('/'));
        }
        let found = match problems.len() {
            1 => "1 problem".to_string(),
            n => format!("{n} problems"),
        };
        format!("the advisories in {shown} do not check ({found}); nothing was signed")
    })
}

/// `advisories --repo DIR --keys DIR SOURCE`: every `DKSA-*.json` of
/// `SOURCE` checked, and signed as targets of the update repository, in
/// one targets version, with an index (ADR 0534). With `--check SOURCE`,
/// only checked.
fn advisories(options: &Options) -> Result<(), String> {
    let [source] = options.operands.as_slice() else {
        return Err("advisories takes the folder of advisories, such as security/advisories".into());
    };
    let sources = load_advisories(Path::new(source))?;
    if options.check {
        let worst = sources.iter().map(|s| s.advisory.severity).max();
        println!(
            "system-release: checked {} in {source}{}",
            advisories_count(sources.len()),
            worst.map_or_else(String::new, |worst| format!(", the worst {}", worst.word()))
        );
        return Ok(());
    }
    let dir = needed(&options.repo, "--repo")?;
    let keys = targets_keys(options, dir)?;
    let advised = updates::set_advisories(dir, &keys, &sources, tuf::time::now())?;
    println!(
        "system-release: {}{}",
        advised_line(&advised, dir),
        if advised.signed { signed_here(&keys) } else { "" }
    );
    Ok(())
}

/// The line that says how many advisories a repository offers, if it
/// offers any.
fn advisories_line(advisories: &[advisory::Entry]) -> Option<String> {
    let newest = advisories.iter().max_by(|a, b| advisory::compare_ids(&a.id, &b.id))?;
    Some(format!("system-release: {}, newest {}", advisories_count(advisories.len()), newest.id))
}

/// The line that says what `channel` offers.
fn channel_line(channel: &updates::Channel) -> String {
    let firmware =
        channel.firmware.as_ref().map_or_else(String::new, |set| format!(", with firmware {set}"));
    format!(
        "system-release: the {} channel offers Dukovo {}{firmware}",
        channel.channel, channel.version
    )
}

/// `refresh --repo DIR`: a timestamp, and a snapshot when it is due.
fn refresh(options: &Options) -> Result<(), String> {
    let dir = needed(&options.repo, "--repo")?;
    let keys = signing_keys(options, dir)?;
    let mut plans = options.plan.as_deref().map(crate::rollout::Plans::load).transpose()?;
    let refreshed = updates::refresh(dir, &keys, plans.as_mut(), tuf::time::now())?;
    println!("system-release: {} {}", dir.display(), say_signed(&refreshed));
    save_plans(options, plans.as_ref())
}

/// Keeps `plans` where `--plan` says, once what they hold is signed.
fn save_plans(options: &Options, plans: Option<&crate::rollout::Plans>) -> Result<(), String> {
    match (&options.plan, plans) {
        (Some(path), Some(plans)) => plans.save(path),
        _ => Ok(()),
    }
}

/// `rollout --repo DIR --online-key SECRET --plan FILE ...`: on the
/// update server, a release's waves paused or resumed, or held for the
/// canaries that are not green as their reports and the releases the
/// channels offer show them, and the rollouts signed again (ADRs 0469
/// and 0533).
fn rollout(options: &Options) -> Result<(), String> {
    let dir = needed(&options.repo, "--repo")?;
    let path = needed(&options.plan, "--plan")?;
    let keys = signing_keys(options, dir)?;
    let mut plans = crate::rollout::Plans::load(path)?;
    let operands: Vec<&str> = options.operands.iter().map(String::as_str).collect();
    let said = match operands.as_slice() {
        ["pause", version, reason] => {
            let plan = plans.releases.get_mut(*version).ok_or_else(|| {
                format!(
                    "no channel has offered {} since the rollouts began",
                    crate::rollout::named(version)
                )
            })?;
            plan.paused = Some((*reason).to_string());
            format!("{} is paused: {reason}", crate::rollout::named(version))
        }
        ["resume", version] => {
            let plan = plans.releases.get_mut(*version).ok_or_else(|| {
                format!(
                    "no channel has offered {} since the rollouts began",
                    crate::rollout::named(version)
                )
            })?;
            plan.paused = None;
            format!("{} goes on", crate::rollout::named(version))
        }
        ["canaries"] => {
            let canaries = needed(&options.canaries, "--canaries")?;
            let reports = needed(&options.reports, "--reports")?;
            let offered = updates::offers(dir, &plans)?;
            let now = options.now.unwrap_or_else(tuf::time::now);
            let standings = verdict::standings(canaries, reports, &offered, now)?;
            plans.red = canary::red(&standings);
            verdict::said(&plans.red)
        }
        ["canary", ..] => return Err(verdict::BY_HAND.into()),
        _ => {
            return Err("rollout takes pause VERSION REASON, resume VERSION or canaries \
                        --canaries FILE --reports DIR"
                .into());
        }
    };
    let refreshed = updates::refresh(dir, &keys, Some(&mut plans), tuf::time::now())?;
    plans.save(path)?;
    println!("system-release: {said}; {} {}", dir.display(), say_signed(&refreshed));
    for (version, plan) in &plans.releases {
        let held = plan.paused.as_ref().map_or_else(String::new, |why| format!(", paused: {why}"));
        let named = crate::rollout::named(version);
        println!("system-release: {named}: wave {} open{held}", plan.open);
    }
    Ok(())
}

fn say_signed(signed: &updates::Refreshed) -> String {
    match signed {
        updates::Refreshed::Timestamp(version) => format!("signed again, timestamp {version}"),
        updates::Refreshed::All { snapshot, timestamp } => {
            format!("signed again, snapshot {snapshot} and timestamp {timestamp}")
        }
    }
}

/// `publish --repo DIR --online-key SECRET INCOMING`: on the update
/// server, what a deploy left in `INCOMING`, signed and read back.
fn publish(options: &Options) -> Result<(), String> {
    let [incoming] = options.operands.as_slice() else {
        return Err("publish takes the folder a deploy copied into".into());
    };
    let dir = needed(&options.repo, "--repo")?;
    let secret_path = needed(&options.online_key, "--online-key")?;
    let secret =
        std::fs::read(secret_path).map_err(|err| format!("{}: {err}", secret_path.display()))?;
    let mut plans = options.plan.as_deref().map(crate::rollout::Plans::load).transpose()?;
    let deployed =
        updates::publish(dir, Path::new(incoming), &secret, plans.as_mut(), tuf::time::now())?;
    save_plans(options, plans.as_ref())?;
    println!(
        "system-release: {} offers {}, newest Dukovo {}, with {}; {}",
        dir.display(),
        count(deployed.releases.len(), "release"),
        deployed.releases.last().map_or("none", String::as_str),
        count(deployed.new_files, "new file"),
        say_signed(&deployed.signed)
    );
    if !deployed.firmware.is_empty() {
        println!("system-release: firmware sets: {}", deployed.firmware.join(", "));
    }
    for channel in &deployed.channels {
        println!("{}", channel_line(channel));
    }
    if let Some(line) = advisories_line(&deployed.advisories) {
        println!("{line}");
    }
    Ok(())
}

/// `canary-receive --socket PATH --canaries FILE --reports DIR`: on the
/// update server, the receiver that keeps each listed canary's newest
/// report, until it is stopped (ADR 0533).
fn canary_receive(options: &Options) -> Result<(), String> {
    let socket = needed(&options.socket, "--socket")?;
    let paths = crate::receiver::Paths {
        canaries: needed(&options.canaries, "--canaries")?.clone(),
        reports: needed(&options.reports, "--reports")?.clone(),
    };
    crate::receiver::serve(socket, &paths)
}

/// `canary --canaries FILE ...`: on the update server, a canary added to
/// the list with its public key, taken off it, or its colour set by hand
/// or given back to its reports (ADR 0533).
fn canary(options: &Options) -> Result<(), String> {
    let path = needed(&options.canaries, "--canaries")?;
    let mut list = Canaries::load(path)?;
    let now = options.now.unwrap_or_else(tuf::time::now);
    let operands: Vec<&str> = options.operands.iter().map(String::as_str).collect();
    let said = match operands.as_slice() {
        ["add", name, key] => {
            if matches!(*name, "add" | "remove" | "red" | "green" | "auto") {
                return Err(format!(
                    "`{name}` is a word of this command: name the canary otherwise"
                ));
            }
            list.add(name, key, now)?;
            format!("canary {name} added; it holds the waves until its first report is green")
        }
        ["remove", name] => {
            list.remove(name)?;
            format!("canary {name} removed; it holds the waves no longer")
        }
        [name, colour @ ("red" | "green"), reason @ ..] => {
            let (colour, said) =
                if *colour == "red" { (Colour::Red, "red") } else { (Colour::Green, "green") };
            let reason = reason.join(" ");
            list.set_override(name, Some((colour, &reason)), now)?;
            format!("canary {name} is {said} by hand: {}", reason.trim())
        }
        [name, "auto"] => {
            list.set_override(name, None, now)?;
            format!("canary {name} goes by its reports again")
        }
        _ => {
            return Err("canary takes add NAME KEY, remove NAME, NAME red REASON, NAME green \
                        REASON or NAME auto"
                .into());
        }
    };
    list.save(path)?;
    println!("system-release: {said}");
    Ok(())
}

/// `canaries show|changed ...`: what the canaries' reports make of the
/// waves, read only. The exit status, which `changed` sets (ADR 0533).
fn canaries(options: &Options) -> Result<i32, String> {
    let operands: Vec<&str> = options.operands.iter().map(String::as_str).collect();
    match operands.as_slice() {
        ["show"] => show(options).map(|()| 0),
        ["changed"] => changed(options),
        _ => Err("canaries takes show or changed".into()),
    }
}

/// The standing of each canary as `--canaries` and `--reports` show it
/// at `--now`, and, with `--repo` and `--plan`, the releases the
/// channels offer and since when, for the rule on a release not taken;
/// and the plans, if `--plan` named them.
fn standings_of(options: &Options) -> Result<(Vec<Standing>, Option<Plans>), String> {
    let canaries = needed(&options.canaries, "--canaries")?;
    let reports = needed(&options.reports, "--reports")?;
    let plans = options.plan.as_deref().map(Plans::load).transpose()?;
    let offered = match (&options.repo, &plans) {
        (Some(dir), Some(plans)) => updates::offers(dir, plans)?,
        (Some(_), None) => {
            return Err(
                "--repo needs --plan: the plan says when a channel first offered a release".into(),
            );
        }
        (None, _) => Vec::new(),
    };
    let now = options.now.unwrap_or_else(tuf::time::now);
    Ok((verdict::standings(canaries, reports, &offered, now)?, plans))
}

/// `canaries show --canaries FILE --reports DIR [--repo DIR --plan FILE]
/// [--json]`: each canary's colour and why, when it last reported, what
/// it runs and a colour set by hand, and, with `--plan`, whether the plan
/// holds the waves for the canaries that are not green.
fn show(options: &Options) -> Result<(), String> {
    let (standings, plans) = standings_of(options)?;
    let held = plans.as_ref().map(|plans| plans.red.as_slice());
    if options.json {
        let shown = verdict::json(&standings, held);
        println!("{}", serde_json::to_string(&shown).map_err(|err| err.to_string())?);
        return Ok(());
    }
    if standings.is_empty() {
        println!("system-release: no canary is listed: nothing holds the waves");
    }
    for standing in &standings {
        println!("system-release: {}", verdict::line(standing));
    }
    if let Some(held) = held {
        let red = canary::red(&standings);
        let step = if verdict::differs(held, &red) {
            "out of step: the next check signs the waves again"
        } else {
            "in step"
        };
        println!("system-release: the plan holds the waves for {}; {step}", verdict::listed(held));
    }
    Ok(())
}

/// `canaries changed --canaries FILE --reports DIR --repo DIR --plan
/// FILE`: 0 and one line when the canaries that are not green are not
/// those the plan holds the waves for, so that the rollouts are to be
/// signed again; [`verdict::UNCHANGED`] and nothing when they are the
/// same.
fn changed(options: &Options) -> Result<i32, String> {
    needed(&options.repo, "--repo")?;
    let (standings, plans) = standings_of(options)?;
    let plans = plans.ok_or("--plan is needed")?;
    let red = canary::red(&standings);
    if !verdict::differs(&plans.red, &red) {
        return Ok(verdict::UNCHANGED);
    }
    println!("system-release: {}", verdict::change_line(&plans.red, &red));
    Ok(0)
}

/// `pack --repo DIR --version VERSION --out FILE`: the update file of one
/// release, read back from the root.
fn pack(options: &Options) -> Result<(), String> {
    let dir = needed(&options.repo, "--repo")?;
    let version = needed(&options.version, "--version")?;
    let out = needed(&options.out, "--out")?;
    let root_path = options.root.clone().unwrap_or_else(|| dir.join("metadata/1.root.json"));
    let root = std::fs::read(&root_path)
        .map_err(|err| format!("the trusted root {}: {err}", root_path.display()))?;
    let packed = updates::pack(dir, &root, version, out)?;
    println!(
        "system-release: Dukovo {version}'s update file, {} files in {} MiB, in {}",
        packed.files,
        packed.bytes >> 20,
        out.display()
    );
    Ok(())
}

/// `check REPOSITORY|FILE`: the release there, read from the trusted root
/// offline, and every one of its files.
fn check(options: &Options) -> Result<(), String> {
    let [dir] = options.operands.as_slice() else {
        return Err(
            "check takes the folder of a release's repository, such as an install stick's \
             release, or an update file"
                .into(),
        );
    };
    let root_path = options.root.clone().unwrap_or_else(|| PathBuf::from(TRUSTED_ROOT));
    let root = std::fs::read(&root_path)
        .map_err(|err| format!("the trusted root {}: {err}", root_path.display()))?;
    let dir = Path::new(dir);
    let (opened, archive) = if dir.is_file() {
        let (opened, archive) = updates::open_file(dir, &root, options.version.as_deref())?;
        (opened, Some(archive))
    } else {
        (repo::open_named(dir, &root, options.version.as_deref())?, None)
    };
    let release = &opened.release;
    println!(
        "system-release: release {} ({}), built {} from commit {}, is signed by the trusted root",
        release.version,
        &opened.name[..16],
        release.built,
        release.commit
    );
    let checked = match &archive {
        Some(archive) => updates::check_archive(archive, release),
        None => repo::check_files(dir, release),
    };
    match checked {
        Ok(checked) => {
            println!(
                "system-release: all {} files match the signed list ({} MiB)",
                checked.files,
                checked.bytes >> 20
            );
            Ok(())
        }
        Err(problems) => {
            for problem in problems.iter().take(SHOWN_PROBLEMS) {
                eprintln!("system-release: {problem}");
            }
            if problems.len() > SHOWN_PROBLEMS {
                eprintln!("system-release: and {} more", problems.len() - SHOWN_PROBLEMS);
            }
            Err(format!(
                "{} of the release's files do not match its signed list: the stick or download is damaged; write it again",
                problems.len()
            ))
        }
    }
}

/// `served URL`: the repository an update server serves there, read as a
/// computer online reads it, from the trusted root: every release list
/// and channel, but none of the files (ADR 0467).
fn served(options: &Options) -> Result<(), String> {
    let [url] = options.operands.as_slice() else {
        return Err("served takes the address of an update server's repository".into());
    };
    let root_path = options.root.clone().unwrap_or_else(|| PathBuf::from(TRUSTED_ROOT));
    let root = std::fs::read(&root_path)
        .map_err(|err| format!("the trusted root {}: {err}", root_path.display()))?;
    let url = if url.ends_with('/') { url.clone() } else { format!("{url}/") };
    let base = tuf::Url::parse(&url).map_err(|err| format!("{url}: {err}"))?;
    if !repo::serves(&base)? {
        if options.json {
            println!("{}", serde_json::json!({ "url": url, "published": false }));
        } else {
            println!("system-release: {url} serves no releases yet: none has been deployed");
        }
        return Ok(());
    }
    if options.json {
        let status = updates::status(&base, &root)?;
        println!("{}", serde_json::to_string(&status).map_err(|err| err.to_string())?);
        return Ok(());
    }
    let offered = updates::served(&base, &root)?;
    println!(
        "system-release: {url} offers {}, newest Dukovo {}, signed from the trusted root",
        count(offered.releases.len(), "release"),
        offered.releases.last().map_or("none", String::as_str)
    );
    if !offered.firmware.is_empty() {
        println!("system-release: firmware sets: {}", offered.firmware.join(", "));
    }
    for channel in &offered.channels {
        println!("{}", channel_line(channel));
    }
    if let Some(line) = advisories_line(&offered.advisories) {
        println!("{line}");
    }
    Ok(())
}

/// `rescue --out ESP REPOSITORY`: Rescue of the release there, read from
/// the trusted root, written into the EFI system partition's folder
/// `ESP`, from the repository's store (ADR 0460).
fn rescue(options: &Options) -> Result<(), String> {
    let [dir] = options.operands.as_slice() else {
        return Err("rescue takes the folder of a release's repository".into());
    };
    let esp = needed(&options.out, "--out")?;
    let root_path = options.root.clone().unwrap_or_else(|| PathBuf::from(TRUSTED_ROOT));
    let root = std::fs::read(&root_path)
        .map_err(|err| format!("the trusted root {}: {err}", root_path.display()))?;
    let dir = Path::new(dir);
    let opened = repo::open_named(dir, &root, options.version.as_deref())?;
    let store = crate::store::Store::at(&dir.join("files"));
    let file = |entry: &crate::list::Entry| store.path(entry.file().map_or("", |(sha, _)| sha));
    let written = crate::rescue::write(esp, dir, &opened.release, &file)?;
    println!(
        "system-release: Rescue of release {}, {} files of {} MiB, in {}",
        opened.release.version,
        written.files,
        written.bytes >> 20,
        esp.join(crate::rescue::DIR).display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::tests::{scratch, tree};

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    #[test]
    fn keys_make_and_check_work_together() {
        let dir = scratch("cli");
        let path = |name: &str| dir.join(name).display().to_string();
        tree(&dir.join("slot"));
        assert_eq!(run(&strings(&["keys", &path("keys")])), 0);
        assert_eq!(run(&strings(&["keys", &path("keys")])), 1);
        let make = [
            "make",
            "--tree",
            &path("slot"),
            "--keys",
            &path("keys"),
            "--out",
            &path("release"),
            "--version",
            "0.14.0",
            "--commit",
            "01d66b7",
            "--toolchain",
            "nightly-2026-10-15",
            "--built",
            "1793923200",
        ];
        assert_eq!(run(&strings(&make)), 0);
        let root = path("keys/root.json");
        assert_eq!(run(&strings(&["check", "--root", &root, &path("release")])), 0);
        assert_eq!(
            run(&strings(&["check", "--root", &root, "--version", "0.13.0", &path("release")])),
            1
        );
        assert_eq!(run(&strings(&["check", "--root", &path("nowhere.json"), &path("release")])), 1);
        let rescue = ["rescue", "--root", &root, "--out", &path("esp"), &path("release")];
        assert_eq!(run(&strings(&rescue)), 0);
        assert!(dir.join("esp/boot/rescue/system").is_file());
        assert_eq!(run(&strings(&["rescue", "--root", &root, &path("release")])), 1);
        assert_eq!(run(&strings(&["make", "--tree"])), 2);
        assert_eq!(run(&strings(&["sign"])), 2);

        // The update repository: add, a channel, refresh, a rotated key,
        // and the update file, checked as a stick is.
        let repo = ["--repo", &path("updates"), "--keys", &path("keys")];
        let with = |words: &[&str]| -> Vec<String> {
            let mut all = strings(&words[..1]);
            all.extend(strings(&repo));
            all.extend(strings(&words[1..]));
            all
        };
        assert_eq!(run(&with(&["add", &path("release")])), 0);
        assert_eq!(run(&with(&["add", &path("release")])), 0);
        assert_eq!(run(&with(&["channel", "stable", "0.14.0"])), 0);
        assert_eq!(run(&with(&["channel", "stable", "0.13.0"])), 1);
        // A firmware set, which the channel then offers beside its release.
        std::fs::write(dir.join("intel-ucode.bin"), b"microcode").unwrap();
        let ucode = path("intel-ucode.bin");
        let firmware = ["firmware", "--version", "2026.11.12", "--microcode", &ucode];
        assert_eq!(run(&with(&[&firmware[..], &["--license", "LicenseRef-Test"]].concat())), 0);
        assert_eq!(run(&with(&firmware)), 1);
        assert_eq!(run(&with(&["channel", "--firmware", "2026.11.11", "stable"])), 1);
        assert_eq!(run(&with(&["channel", "--firmware", "2026.11.12", "beta"])), 1);
        assert_eq!(run(&with(&["channel", "--firmware", "2026.11.12", "stable"])), 0);
        let firmware =
            crate::firmware::open(&dir.join("updates"), &std::fs::read(&root).unwrap(), None);
        assert_eq!(firmware.unwrap().0.files[0].size, 9);
        assert_eq!(run(&strings(&["keys", "--rotate", "snapshot", &path("keys")])), 0);
        assert_eq!(run(&with(&["refresh"])), 0);
        let served =
            format!("file://{}", std::path::absolute(dir.join("updates")).unwrap().display());
        assert_eq!(run(&strings(&["served", "--root", &root, &served])), 0);
        assert_eq!(run(&strings(&["served", "--json", "--root", &root, &served])), 0);
        // A server that has had nothing deployed has no timestamp, and is
        // not at fault; one that cannot be reached is.
        assert!(!repo::serves(&tuf::Url::parse("file:///nowhere/").unwrap()).unwrap());
        assert!(repo::serves(&tuf::Url::parse(&format!("{served}/")).unwrap()).unwrap());
        assert_eq!(run(&strings(&["served", "--root", &root, "file:///nowhere"])), 0);
        assert_eq!(run(&strings(&["served", "--json", "--root", &root, "file:///nowhere"])), 0);
        assert_eq!(run(&strings(&["served", "--root", &root, "ftp://nowhere/"])), 1);

        // What the status read says: the roles, the release and firmware
        // set the channel offers, and their rollouts, once the server has
        // signed them, and a pause and a withdrawal.
        let base = tuf::Url::parse(&format!("{served}/")).unwrap();
        let root_bytes = std::fs::read(&root).unwrap();
        let status = updates::status(&base, &root_bytes).unwrap();
        let roles: Vec<&str> = status.roles.iter().map(|r| r.role.as_str()).collect();
        assert_eq!(roles, ["root", "timestamp", "snapshot", "targets", "rollout"]);
        let now = tuf::time::now();
        assert!(status.roles.iter().all(|r| r.version >= 1 && r.expires > now));
        assert!(status.checked >= now - 5);
        assert_eq!(status.releases.len(), 1);
        assert_eq!(status.releases[0].version, "0.14.0");
        assert!(status.releases[0].rollout.is_none() && status.releases[0].withdrawn.is_none());
        assert_eq!(status.firmware[0].version, "2026.11.12");
        assert_eq!(
            status.channels.iter().map(|c| c.firmware.as_deref()).collect::<Vec<_>>(),
            [Some("2026.11.12")]
        );
        let plan = path("plan.json");
        assert_eq!(run(&with(&["refresh", "--plan", &plan])), 0);
        assert_eq!(run(&with(&["rollout", "--plan", &plan, "pause", "0.14.0", "disk errors"])), 0);
        let status = updates::status(&base, &root_bytes).unwrap();
        let release = &status.releases[0];
        let rollout = release.rollout.as_ref().unwrap();
        assert_eq!(rollout.held.as_deref(), Some("paused: disk errors"));
        assert!(status.firmware[0].rollout.as_ref().unwrap().held.is_none());
        let json: serde_json::Value = serde_json::to_value(&status).unwrap();
        assert_eq!(json["channels"][0]["channel"], "stable");
        assert!(json["releases"][0]["withdrawn"].is_null());
        let pull = ["pull", "--version", "0.14.0", "--reason", "boots badly", "--advice", "stay"];
        assert_eq!(run(&with(&pull)), 0);
        let status = updates::status(&base, &root_bytes).unwrap();
        let withdrawn = status.releases[0].withdrawn.as_ref().unwrap();
        assert_eq!(withdrawn.reason, "boots badly");
        let json: serde_json::Value = serde_json::to_value(&status).unwrap();
        assert_eq!(json["releases"][0]["withdrawn"]["advice"], "stay");
        let file = path("dukovo-0.14.0.update");
        let pack = ["pack", "--repo", &path("updates"), "--version", "0.14.0", "--out", &file];
        assert_eq!(run(&strings(&pack)), 0);
        assert_eq!(run(&strings(&["check", "--root", &root, &file])), 0);
        assert_eq!(run(&strings(&["check", "--root", &root, &path("updates")])), 0);
        assert_eq!(
            run(&strings(&["check", "--root", &root, "--version", "0.13.0", &path("updates")])),
            1
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The canary `ref-box`'s key and the list that names it, in `dir`.
    fn listed_canary(dir: &Path) -> (crate::canary::Key, String) {
        let key = crate::canary::Key::from_pkcs8(&crate::canary::Key::generate().unwrap()).unwrap();
        let list = dir.join("canaries.json").display().to_string();
        let add = ["canary", "--canaries", &list, "add", "ref-box", &key.public_hex()];
        assert_eq!(run(&strings(&add)), 0);
        (key, list)
    }

    #[test]
    fn advisories_are_checked_and_signed_alone_or_with_a_channel() {
        let dir = scratch("cli-advisories");
        let (_, repo) = offering(&dir);
        let with = |words: &[&str]| -> Vec<String> {
            let mut all = strings(&words[..1]);
            all.extend(repo.clone());
            all.extend(strings(&words[1..]));
            all
        };
        let folder = |name: &str, advisories: &[(&str, &str, &str)]| {
            crate::advisory::tests::folder(&dir.join(name), advisories);
            dir.join(name).display().to_string()
        };
        let targets = || {
            std::fs::read_dir(dir.join("updates/metadata"))
                .unwrap()
                .filter(|entry| {
                    entry.as_ref().unwrap().file_name().to_string_lossy().ends_with(".targets.json")
                })
                .count()
        };
        let read = || {
            let url =
                format!("file://{}/", std::path::absolute(dir.join("updates")).unwrap().display());
            let root = std::fs::read(dir.join("keys/root.json")).unwrap();
            updates::status(&tuf::Url::parse(&url).unwrap(), &root).unwrap()
        };
        let two = folder(
            "two",
            &[
                ("DKSA-2026-0001", "2026-11-15T09:30:00Z", "high"),
                ("DKSA-2026-0002", "2026-11-16T09:30:00Z", "low"),
            ],
        );

        // Checked alone: no repository or keys are needed.
        assert_eq!(run(&strings(&["advisories", "--check", &two])), 0);
        assert_eq!(read().advisories.len(), 0);
        assert_eq!(run(&strings(&["advisories", "--check"])), 1);
        assert_eq!(
            run(&strings(&["advisories", "--check", &dir.join("nowhere").display().to_string()])),
            1
        );

        // Signed: the status lists them, newest id last, with their
        // severity, and the same again signs no new targets.
        let before = targets();
        assert_eq!(run(&with(&["advisories", &two])), 0);
        assert_eq!(targets(), before + 1);
        let status = read();
        assert_eq!(status.advisories.len(), 2);
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["advisories"][0]["id"], "DKSA-2026-0001");
        assert_eq!(json["advisories"][0]["severity"], "high");
        assert_eq!(json["advisories"][1]["severity"], "low");
        assert_eq!(run(&with(&["advisories", &two])), 0);
        assert_eq!(targets(), before + 1);

        // An advisory that does not check stops it all, and the others are
        // not signed.
        let bad = folder("bad", &[("DKSA-2026-0003", "2026-12-01T00:00:00Z", "low")]);
        let mut value: serde_json::Value =
            serde_json::from_str(crate::advisory::tests::FIXTURE).unwrap();
        value["id"] = "DKSA-2026-0004".into();
        value["summary"] = "TODO: what is wrong".into();
        std::fs::write(dir.join("bad/DKSA-2026-0004.json"), serde_json::to_vec(&value).unwrap())
            .unwrap();
        assert_eq!(run(&strings(&["advisories", "--check", &bad])), 1);
        assert_eq!(run(&with(&["advisories", &bad])), 1);
        assert_eq!(targets(), before + 1);
        assert_eq!(read().advisories.len(), 2);
        assert_eq!(run(&with(&["advisories"])), 1);
        assert_eq!(run(&strings(&["advisories", &two])), 1);

        // A channel moved with --advisories signs them in its one version.
        let one = folder("one", &[("DKSA-2026-0001", "2026-11-15T09:30:00Z", "high")]);
        assert_eq!(run(&with(&["channel", "--advisories", &bad, "beta", "0.14.0"])), 1);
        assert_eq!(targets(), before + 1);
        assert_eq!(run(&with(&["channel", "--advisories", &one, "beta", "0.14.0"])), 0);
        assert_eq!(targets(), before + 2);
        let status = read();
        assert_eq!(status.advisories.len(), 1);
        assert_eq!(status.channels.len(), 2);
        let nowhere = dir.join("nowhere").display().to_string();
        assert_eq!(run(&with(&["channel", "--advisories", &nowhere, "beta", "0.14.0"])), 1);
        assert_eq!(targets(), before + 2);
        // Without the option, the channel leaves the advisories as they are.
        assert_eq!(run(&with(&["channel", "stable", "0.14.0"])), 0);
        assert_eq!(read().advisories.len(), 1);
        // The server's reading, and the line `served` prints, name them.
        let url = format!("file://{}", std::path::absolute(dir.join("updates")).unwrap().display());
        let root = dir.join("keys/root.json").display().to_string();
        assert_eq!(run(&strings(&["served", "--root", &root, &url])), 0);
        let line = advisories_line(&read().advisories).unwrap();
        assert_eq!(line, "system-release: 1 security advisory, newest DKSA-2026-0001");
        assert_eq!(advisories_line(&[]), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_canary_list_is_changed_by_hand_one_line_at_a_time() {
        let dir = scratch("cli-canary");
        let (key, list) = listed_canary(&dir);
        let canary = |words: &[&str]| {
            let mut all = strings(&["canary", "--canaries", &list]);
            all.extend(strings(words));
            run(&all)
        };
        let read = || crate::canary::Canaries::load(Path::new(&list)).unwrap();
        assert_eq!(read().canaries["ref-box"].key, key.public_hex());
        // A name or key twice, a word of the command as a name, a bad key.
        assert_eq!(canary(&["add", "ref-box", &key.public_hex()]), 1);
        assert_eq!(canary(&["add", "other", &key.public_hex()]), 1);
        assert_eq!(canary(&["add", "auto", &"ab".repeat(32)]), 1);
        assert_eq!(canary(&["add", "other", "abcd"]), 1);
        // A colour by hand needs its reason, which may be several words.
        assert_eq!(canary(&["ref-box", "red"]), 1);
        assert_eq!(canary(&["ref-box", "red", "disk", "swap"]), 0);
        let over = read().canaries["ref-box"].over.clone().unwrap();
        assert_eq!((over.colour, over.reason.as_str()), (Colour::Red, "disk swap"));
        assert_eq!(canary(&["ref-box", "green", "swapped"]), 0);
        assert_eq!(read().canaries["ref-box"].over.as_ref().unwrap().colour, Colour::Green);
        assert_eq!(canary(&["ref-box", "auto"]), 0);
        assert!(read().canaries["ref-box"].over.is_none());
        assert_eq!(canary(&["nobody", "auto"]), 1);
        assert_eq!(canary(&["nobody", "red", "why"]), 1);
        assert_eq!(canary(&["ref-box", "blue", "why"]), 1);
        assert_eq!(canary(&["remove", "nobody"]), 1);
        assert_eq!(canary(&["remove", "ref-box"]), 0);
        assert!(read().canaries.is_empty());
        assert_eq!(canary(&[]), 1);
        assert_eq!(run(&strings(&["canary", "add", "ref-box", &key.public_hex()])), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fetch_and_offer_sign_a_release_on_a_yubikey_with_its_rekor_entry() {
        let dir = scratch("cli-offer");
        let path = |name: &str| dir.join(name).display().to_string();
        let (helper, _, _) = crate::updates::tests::ceremony(&dir);
        let (helper, root) = (helper.display().to_string(), path("releases/metadata/1.root.json"));
        tree(&dir.join("slot"));
        let list = strings(&["list", "--tree", &path("slot"), "--version", "0.18.0"]);
        let about = ["--commit", "01d66b7", "--toolchain", "nightly-2026-10-15"];
        let out = ["--built", "1793923200", "--out", &path("system.json")];
        assert_eq!(run(&[list, strings(&about), strings(&out)].concat()), 0);
        let advisories = crate::advisory::tests::folder(
            &dir.join("advisories"),
            &[("DKSA-2026-0001", "2026-11-15T09:30:00Z", "high")],
        );
        assert_eq!(advisories.len(), 1);
        let server = format!("file://{}/", path("server"));

        let fetch = strings(&["fetch", "--root", &root, "--out", &path("work"), &server]);
        assert_eq!(run(&fetch), 0);
        assert_eq!(run(&fetch), 1);
        // `--yes` in place of the version typed, which a test cannot type.
        let offer = ["offer", "--repo", &path("work"), "--tree", &path("slot"), "--yes"];
        let signer = ["--yubikey-helper", &helper, "--serial", "1001"];
        let record = ["--record", &path("rekor.json"), "--advisories", &path("advisories")];
        let operands = [&path("system.json"), "beta"];
        assert_eq!(run(&strings(&[&offer[..], &signer, &record, &operands].concat())), 0);
        assert!(dir.join("work/metadata/1.targets.json").exists());
        let entry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("rekor.json")).unwrap()).unwrap();
        assert_eq!(entry["kind"], "hashedrekord");
        // The release keys of the development folder do not make a record.
        assert_eq!(run(&strings(&["keys", &path("keys")])), 0);
        let dev = ["offer", "--repo", &path("work"), "--keys", &path("keys"), "--record", "x"];
        assert_eq!(run(&strings(&[&dev[..], &[&path("system.json"), "beta"]].concat())), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_served_release_is_laid_out_for_a_medium_and_its_images_checksums_signed() {
        let dir = scratch("cli-medium");
        let path = |name: &str| dir.join(name).display().to_string();
        let (helper, _, secret) = crate::updates::tests::ceremony(&dir);
        let (helper, root) = (helper.display().to_string(), path("releases/metadata/1.root.json"));
        tree(&dir.join("slot"));
        let list = strings(&["list", "--tree", &path("slot"), "--version", "0.18.0"]);
        let about = ["--commit", "01d66b7", "--toolchain", "nightly-2026-10-15"];
        let out = ["--built", "1793923200", "--out", &path("system.json")];
        assert_eq!(run(&[list, strings(&about), strings(&out)].concat()), 0);
        let server = format!("file://{}/", path("server"));
        let medium = ["medium", "--root", &root, "--tree", &path("slot"), "--out", &path("medium")];
        let medium = strings(&[&medium[..], &[&server, &path("system.json")]].concat());
        // Nothing is served yet.
        assert_eq!(run(&medium), 1);
        assert_eq!(run(&strings(&["fetch", "--root", &root, "--out", &path("work"), &server])), 0);
        let offer = ["offer", "--repo", &path("work"), "--tree", &path("slot"), "--yes"];
        let signer = ["--yubikey-helper", &helper, "--serial", "1001"];
        let operands = [&path("system.json"), "beta"];
        assert_eq!(run(&strings(&[&offer[..], &signer, &operands].concat())), 0);
        let now = tuf::time::now();
        updates::publish(&dir.join("server"), &dir.join("work"), &secret, None, now).unwrap();

        assert_eq!(run(&medium), 0);
        assert_eq!(run(&strings(&["check", "--root", &root, &path("medium")])), 0);
        let images = dir.join("images");
        std::fs::create_dir_all(&images).unwrap();
        for name in ["dukovo-0.18.0-x86_64.img", "dukovo-0.18.0-x86_64.iso"] {
            std::fs::write(images.join(name), name).unwrap();
        }
        let image = |name: &str| images.join(name).display().to_string();
        let sums = [
            "sums",
            "--root",
            &path("medium/metadata/root.json"),
            "--version",
            "0.18.0",
            "--out",
            &path("images"),
            "--record",
            &path("sums-rekor.json"),
            "--yes",
        ];
        let named = [image("dukovo-0.18.0-x86_64.img"), image("dukovo-0.18.0-x86_64.iso")];
        let named: Vec<&str> = named.iter().map(String::as_str).collect();
        assert_eq!(run(&strings(&[&sums[..], &signer, &named].concat())), 0);
        assert!(images.join("SHA256SUMS").is_file());
        assert!(images.join("SHA256SUMS.yubikey-1001.sig").is_file());
        let entry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("sums-rekor.json")).unwrap()).unwrap();
        assert_eq!(entry["kind"], "hashedrekord");
        // Images are named; the checksums of none are not signed.
        assert_eq!(run(&strings(&[&sums[..], &signer].concat())), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_is_to_be_signed_is_shown_whole() {
        let mut proposal = updates::Proposal {
            version: "1.0.0-alpha.1".into(),
            name: "ab".repeat(32),
            commit: "53299fc".into(),
            files: 1,
            bytes: 3 << 20,
            channel: "beta".into(),
            replaces: None,
            targets_version: 4,
            expires: "2027-05-27T00:00:00Z".into(),
            advised: None,
        };
        let shown = proposal_lines(&proposal);
        assert!(shown.contains("Dukovo 1.0.0-alpha.1, release list SHA-256 abab"), "{shown}");
        assert!(shown.contains("commit 53299fc, 1 file of 3 MiB"), "{shown}");
        assert!(shown.contains("beta channel, which offers no release until now"), "{shown}");
        assert!(shown.contains("no security advisories"), "{shown}");
        assert!(shown.contains("targets version 4, which expires 2027-05-27T00:00:00Z"), "{shown}");
        proposal.replaces = Some("0.18.0".into());
        proposal.advised = Some(updates::Advised {
            count: 2,
            added: vec!["DKSA-2026-0002".into()],
            ..updates::Advised::default()
        });
        let shown = proposal_lines(&proposal);
        assert!(shown.contains("beta channel, in place of 0.18.0"), "{shown}");
        assert!(shown.contains("2 security advisories, new: DKSA-2026-0002"), "{shown}");
    }

    #[test]
    fn the_checksums_to_be_signed_are_shown_with_the_yubikey_that_signs() {
        use tuf::repo::Signer as _;
        let key = crate::ceremony::tests::P256::new().key().clone();
        let id = key.id();
        let yubikey = crate::yubikey::YubiKey::new(Path::new("helper"), 38_775_519, "95", key);
        let image = |name: &str, bytes| crate::sums::Image {
            name: name.into(),
            bytes,
            sha256: "cd".repeat(32),
        };
        let images = [image("dukovo-1.0.0-alpha.1-x86_64.img", 3 << 20), image("x.iso", 1)];
        let shown = sums_lines("1.0.0-alpha.1", &images, &yubikey);
        let signer = format!("YubiKey 38775519, with release key {}", &id[..16]);
        assert!(shown.contains(&signer), "{shown}");
        assert!(shown.contains("install images of Dukovo 1.0.0-alpha.1"), "{shown}");
        assert!(shown.contains("dukovo-1.0.0-alpha.1-x86_64.img, 3 MiB, SHA-256 cdcd"), "{shown}");
        assert!(shown.contains("x.iso, 1 MiB"), "{shown}");
    }

    #[test]
    fn list_writes_the_unsigned_list_that_make_signs_and_needs_no_keys() {
        let dir = scratch("list");
        let path = |name: &str| dir.join(name).display().to_string();
        tree(&dir.join("slot"));
        let words = |extra: &[&str]| {
            let mut all = strings(&[
                "--tree",
                &path("slot"),
                "--version",
                "0.14.0",
                "--commit",
                "01d66b7",
                "--toolchain",
                "nightly-2026-10-15",
                "--built",
                "1793923200",
            ]);
            all.extend(strings(extra));
            all
        };
        // No keys anywhere: a list is made, twice the same.
        let list = |out: &str| {
            let mut args = strings(&["list"]);
            args.extend(words(&["--out", &path(out)]));
            assert_eq!(run(&args), 0);
            std::fs::read(dir.join(out)).unwrap()
        };
        let (first, second) = (list("first.json"), list("second.json"));
        assert_eq!(first, second);

        // It is the list `make` signs: the same bytes as the target in the
        // repository `make` lays out.
        assert_eq!(run(&strings(&["keys", &path("keys")])), 0);
        let mut make = strings(&["make", "--keys", &path("keys"), "--out", &path("release")]);
        make.extend(words(&[]));
        assert_eq!(run(&make), 0);
        let targets = dir.join("release/targets/system");
        let signed = std::fs::read_dir(&targets).unwrap().next().unwrap().unwrap().path();
        assert_eq!(first, std::fs::read(signed).unwrap());
        let release = Release::parse(&first).unwrap();
        assert_eq!((release.version.as_str(), release.commit.as_str()), ("0.14.0", "01d66b7"));

        // A tree, a version, a commit, a toolchain and a time are needed.
        assert_eq!(run(&strings(&["list", "--tree", &path("slot")])), 1);
        assert_eq!(run(&strings(&["list", "--tree", &path("nowhere")])), 1);
        let mut missing = strings(&["list"]);
        missing.extend(words(&[]).into_iter().take(6));
        assert_eq!(run(&missing), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An update repository `updates` in `dir` offering release 0.14.0 on
    /// stable, with the keys in `keys`, whose plan is `plan`; and the
    /// arguments that name the repository and its keys.
    fn offering(dir: &Path) -> (String, Vec<String>) {
        let path = |name: &str| dir.join(name).display().to_string();
        tree(&dir.join("slot"));
        assert_eq!(run(&strings(&["keys", &path("keys")])), 0);
        let make = [
            "make",
            "--tree",
            &path("slot"),
            "--keys",
            &path("keys"),
            "--out",
            &path("release"),
            "--version",
            "0.14.0",
            "--commit",
            "01d66b7",
            "--toolchain",
            "nightly-2026-10-15",
            "--built",
            "1793923200",
        ];
        assert_eq!(run(&strings(&make)), 0);
        let repo = strings(&["--repo", &path("updates"), "--keys", &path("keys")]);
        let with = |words: &[&str]| -> Vec<String> {
            let mut all = strings(&words[..1]);
            all.extend(repo.clone());
            all.extend(strings(&words[1..]));
            all
        };
        assert_eq!(run(&with(&["add", &path("release")])), 0);
        assert_eq!(run(&with(&["channel", "stable", "0.14.0"])), 0);
        let plan = path("plan.json");
        assert_eq!(run(&with(&["refresh", "--plan", &plan])), 0);
        (plan, repo)
    }

    #[test]
    fn the_canaries_hold_the_waves_and_the_plan_follows_them() {
        let dir = scratch("cli-canaries");
        let (plan, repo) = offering(&dir);
        let (key, list) = listed_canary(&dir);
        let reports = dir.join("reports");
        std::fs::create_dir(&reports).unwrap();
        let reports_arg = reports.display().to_string();
        let now = tuf::time::now();
        let with_repo = |words: &[&str]| -> Vec<String> {
            let mut all = strings(&words[..1]);
            all.extend(repo.clone());
            all.extend(strings(&["--plan", &plan]));
            all.extend(strings(&words[1..]));
            all
        };
        let canaries_args = |now: i64| -> Vec<String> {
            strings(&["--canaries", &list, "--reports", &reports_arg, "--now", &now.to_string()])
        };
        // `changed`: 0 when the rollouts are to be signed again, 3 when
        // they are not, 1 when it cannot say.
        let changed = |now: i64| {
            let mut all = strings(&["canaries", "changed"]);
            all.extend(canaries_args(now));
            all.extend(strings(&["--repo", &repo[1], "--plan", &plan]));
            run(&all)
        };
        let rollout = |now: i64| {
            let mut all = with_repo(&["rollout"]);
            all.push("canaries".into());
            all.extend(canaries_args(now));
            run(&all)
        };
        let held = |dir: &Path| {
            let base = tuf::Url::parse(&format!(
                "file://{}/",
                std::path::absolute(dir.join("updates")).unwrap().display()
            ))
            .unwrap();
            let root = std::fs::read(dir.join("keys/root.json")).unwrap();
            let status = updates::status(&base, &root).unwrap();
            status.releases[0].rollout.as_ref().and_then(|r| r.held.clone())
        };
        let seal = |time: i64, healthy: bool| {
            let mut report = crate::canary::Report::new("ref-box", time, "0.14.0", "stable");
            report.health =
                Some(crate::canary::Health { passed: healthy, time, detail: String::new() });
            report.check = Some(crate::canary::Check { failures: 0, warnings: 0, failed: vec![] });
            std::fs::write(reports.join("ref-box.json"), key.seal(&report)).unwrap();
        };

        // A canary that has not reported holds the waves; signing again
        // says so, and then nothing is to be signed.
        assert_eq!(changed(now), 0);
        assert_eq!(rollout(now), 0);
        assert_eq!(held(&dir).as_deref(), Some("canaries not green: ref-box"));
        assert_eq!(Plans::load(Path::new(&plan)).unwrap().red, ["ref-box"]);
        assert_eq!(changed(now), verdict::UNCHANGED);
        // Its first healthy report frees them.
        seal(now - 60, true);
        assert_eq!(changed(now), 0);
        assert_eq!(rollout(now), 0);
        assert_eq!(held(&dir), None);
        assert_eq!(changed(now), verdict::UNCHANGED);
        // Three hours on, with no report since, it is silent.
        assert_eq!(changed(now + 3 * 3600), 0);
        // A failed health check is red, and a report of the canary that
        // is newer and healthy is green.
        seal(now, false);
        assert_eq!(changed(now), 0);
        assert_eq!(rollout(now), 0);
        assert_eq!(held(&dir).as_deref(), Some("canaries not green: ref-box"));
        // A colour set by hand wins, until it is taken away.
        let by_hand = |words: &[&str]| {
            let mut all = strings(&["canary", "--canaries", &list]);
            all.extend(strings(words));
            run(&all)
        };
        assert_eq!(by_hand(&["ref-box", "green", "known"]), 0);
        assert_eq!(changed(now), 0);
        assert_eq!(rollout(now), 0);
        assert_eq!(held(&dir), None);
        assert_eq!(by_hand(&["ref-box", "auto"]), 0);
        assert_eq!(changed(now), 0);
        // Taking the canary off the list frees the waves.
        assert_eq!(rollout(now), 0);
        assert_eq!(by_hand(&["remove", "ref-box"]), 0);
        assert_eq!(changed(now), 0);
        assert_eq!(rollout(now), 0);
        assert_eq!(held(&dir), None);
        assert_eq!(changed(now), verdict::UNCHANGED);
        // `changed` needs the repository and the plan.
        let mut bare = strings(&["canaries", "changed"]);
        bare.extend(canaries_args(now));
        assert_eq!(run(&bare), 1);
        // The old way of marking a canary names the new one.
        assert_eq!(run(&with_repo(&["rollout", "canary", "ref-box", "red"])), 1);
        assert_eq!(run(&strings(&["canaries", "sleep"])), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn show_says_each_canarys_standing_and_whether_the_plan_is_in_step() {
        let dir = scratch("cli-show");
        let (plan, repo) = offering(&dir);
        let (key, list) = listed_canary(&dir);
        let reports = dir.join("reports");
        std::fs::create_dir(&reports).unwrap();
        let (reports_arg, now) = (reports.display().to_string(), tuf::time::now());
        let args = |extra: &[&str]| -> Options {
            let mut all = strings(&["--canaries", &list, "--reports", &reports_arg]);
            all.extend(strings(extra));
            Options::parse(&all).unwrap()
        };
        let (standings, plans) = standings_of(&args(&[])).unwrap();
        assert!(plans.is_none());
        assert_eq!(standings.len(), 1);
        assert!(!standings[0].green);
        assert_eq!(standings[0].reasons, ["no report since it was added"]);
        assert!(verdict::json(&standings, None).get("in_step").is_none());
        // With the plan, which holds nothing, it is out of step.
        let (standings, plans) =
            standings_of(&args(&["--plan", &plan, "--repo", &repo[1]])).unwrap();
        let held = plans.as_ref().map(|p| p.red.as_slice());
        let shown = verdict::json(&standings, held);
        assert_eq!(shown["in_step"], false);
        assert_eq!(shown["red"][0], "ref-box");
        // A report that is fresh and healthy, at an evaluation time given.
        let mut report = crate::canary::Report::new("ref-box", now - 5, "0.14.0", "stable");
        report.check = Some(crate::canary::Check { failures: 0, warnings: 0, failed: vec![] });
        std::fs::write(reports.join("ref-box.json"), key.seal(&report)).unwrap();
        let (standings, _) = standings_of(&args(&["--now", &now.to_string()])).unwrap();
        assert!(standings[0].green, "{:?}", standings[0].reasons);
        let line = verdict::line(&standings[0]);
        assert!(line.starts_with("ref-box: green; last report "), "{line}");
        assert!(line.ends_with("running 0.14.0 on stable"), "{line}");
        // The repository without the plan is a mistake, not a guess.
        assert!(standings_of(&args(&["--repo", &repo[1]])).is_err());
        // Both forms print and succeed.
        let mut show =
            strings(&["canaries", "show", "--canaries", &list, "--reports", &reports_arg]);
        assert_eq!(run(&show), 0);
        show.extend(strings(&["--json", "--plan", &plan, "--repo", &repo[1]]));
        assert_eq!(run(&show), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
