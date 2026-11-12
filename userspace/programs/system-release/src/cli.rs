// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The system-release command (system-release(8)): `ob` runs it on the
//! build machine to make keys and sign releases and the update
//! repository that offers them, and Dukovo has it as one of the native
//! tools, to check an install stick's release or an update file.

use std::path::{Path, PathBuf};

use crate::list::{About, Release};
use crate::repo::{self, Keys};
use crate::updates;

/// Where a Dukovo system keeps the root of the releases it trusts.
pub const TRUSTED_ROOT: &str = "/system/resources/updates/root.json";

const USAGE: &str = "\
usage: system-release keys [--rotate ROLE | --online KEY] DIR
       system-release online-key --out KEY SECRET
       system-release make --tree DIR --keys DIR --out DIR --version VERSION
                           --commit COMMIT --toolchain TOOLCHAIN --built SECONDS
                           [--updates-from VERSION]
       system-release add --repo DIR --keys DIR RELEASE
       system-release firmware --repo DIR --keys DIR --version VERSION
                               --microcode FILE --license LICENSE
       system-release pull --repo DIR --keys DIR --version VERSION --reason TEXT
                           --advice rollback|stay
       system-release channel --repo DIR --keys DIR CHANNEL VERSION
       system-release channel --repo DIR --keys DIR --firmware VERSION|none CHANNEL
       system-release refresh --repo DIR (--keys DIR | --online-key SECRET [--plan FILE])
       system-release publish --repo DIR --online-key SECRET [--plan FILE] INCOMING
       system-release rollout --repo DIR --online-key SECRET --plan FILE
                              (pause VERSION REASON | resume VERSION | canary NAME red|green)
       system-release pack --repo DIR [--root FILE] --version VERSION --out FILE
       system-release check [--root FILE] [--version VERSION] REPOSITORY|FILE
       system-release served [--root FILE] URL
       system-release rescue --out ESP [--root FILE] [--version VERSION] REPOSITORY";

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
        "make" => make(&options),
        "add" => add(&options),
        "firmware" => firmware(&options),
        "pull" => pull(&options),
        "channel" => channel(&options),
        "refresh" => refresh(&options),
        "publish" => publish(&options),
        "rollout" => rollout(&options),
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
    reason: Option<String>,
    advice: Option<String>,
    plan: Option<PathBuf>,
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
                "--reason" => options.reason = Some(value()?),
                "--advice" => options.advice = Some(value()?),
                "--plan" => options.plan = Some(value()?.into()),
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
    let keys = Keys::load(needed(&options.keys, "--keys")?)?;
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
/// with `--firmware VERSION|none CHANNEL`, the firmware set, or none.
fn channel(options: &Options) -> Result<(), String> {
    let dir = needed(&options.repo, "--repo")?;
    let keys = Keys::load(needed(&options.keys, "--keys")?)?;
    if let Some(firmware) = &options.firmware {
        let [channel] = options.operands.as_slice() else {
            return Err("channel --firmware takes the channel".into());
        };
        let set = (firmware != "none").then_some(firmware.as_str());
        updates::set_channel_firmware(dir, &keys, channel, set, tuf::time::now())?;
        let offers = set.map_or_else(|| "no firmware set".into(), |set| format!("firmware {set}"));
        println!(
            "system-release: the {channel} channel of {} offers {offers}{}",
            dir.display(),
            signed_here(&keys)
        );
        return Ok(());
    }
    let [channel, version] = options.operands.as_slice() else {
        return Err("channel takes the channel and the version it is to offer".into());
    };
    updates::set_channel(dir, &keys, channel, version, tuf::time::now())?;
    println!(
        "system-release: the {channel} channel of {} offers Dukovo {version}{}",
        dir.display(),
        signed_here(&keys)
    );
    Ok(())
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
/// update server, a release's waves paused or resumed, or a canary red
/// or green, and the rollouts signed again (ADR 0469).
fn rollout(options: &Options) -> Result<(), String> {
    let dir = needed(&options.repo, "--repo")?;
    let path = needed(&options.plan, "--plan")?;
    let keys = signing_keys(options, dir)?;
    let mut plans = crate::rollout::Plans::load(path)?;
    let operands: Vec<&str> = options.operands.iter().map(String::as_str).collect();
    let said = match operands.as_slice() {
        ["pause", version, reason] => {
            let plan = plans.releases.get_mut(*version).ok_or_else(|| {
                format!("no channel has offered Dukovo {version} since the rollouts began")
            })?;
            plan.paused = Some((*reason).to_string());
            format!("Dukovo {version} is paused: {reason}")
        }
        ["resume", version] => {
            let plan = plans.releases.get_mut(*version).ok_or_else(|| {
                format!("no channel has offered Dukovo {version} since the rollouts began")
            })?;
            plan.paused = None;
            format!("Dukovo {version} goes on")
        }
        ["canary", name, colour @ ("red" | "green")] => {
            plans.red.retain(|red| red != name);
            if *colour == "red" {
                plans.red.push((*name).to_string());
            }
            format!("the canary {name} is {colour}")
        }
        _ => {
            return Err(
                "rollout takes pause VERSION REASON, resume VERSION or canary NAME red|green"
                    .into(),
            );
        }
    };
    let refreshed = updates::refresh(dir, &keys, Some(&mut plans), tuf::time::now())?;
    plans.save(path)?;
    println!("system-release: {said}; {} {}", dir.display(), say_signed(&refreshed));
    for (version, plan) in &plans.releases {
        let held = plan.paused.as_ref().map_or_else(String::new, |why| format!(", paused: {why}"));
        println!("system-release: Dukovo {version}: wave {} open{held}", plan.open);
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
    Ok(())
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
        (repo::open(dir, &root, options.version.as_deref())?, None)
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
    let opened = repo::open(dir, &root, options.version.as_deref())?;
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
        assert_eq!(run(&strings(&["served", "--root", &root, "file:///nowhere"])), 1);
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
}
