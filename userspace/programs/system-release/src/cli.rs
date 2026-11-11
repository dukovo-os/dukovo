// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The system-release command (system-release(8)): `ob` runs it on the
//! build machine to make keys and sign releases, and Dukovo has it as one
//! of the native tools, to check an install stick's release.

use std::path::{Path, PathBuf};

use crate::list::{About, Release};
use crate::repo::{self, Keys};

/// Where a Dukovo system keeps the root of the releases it trusts.
pub const TRUSTED_ROOT: &str = "/system/resources/updates/root.json";

const USAGE: &str = "\
usage: system-release keys DIR
       system-release make --tree DIR --keys DIR --out DIR --version VERSION
                           --commit COMMIT --toolchain TOOLCHAIN --built SECONDS
                           [--updates-from VERSION]
       system-release check [--root FILE] [--version VERSION] REPOSITORY
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
        "make" => make(&options),
        "check" => check(&options),
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
    out: Option<PathBuf>,
    root: Option<PathBuf>,
    version: Option<String>,
    commit: Option<String>,
    toolchain: Option<String>,
    built: Option<i64>,
    updates_from: Option<String>,
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
                "--out" => options.out = Some(value()?.into()),
                "--root" => options.root = Some(value()?.into()),
                "--version" => options.version = Some(value()?),
                "--commit" => options.commit = Some(value()?),
                "--toolchain" => options.toolchain = Some(value()?),
                "--updates-from" => options.updates_from = Some(value()?),
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

/// Gives `option`'s value, or says it is needed.
fn needed<'a, T>(option: &'a Option<T>, name: &str) -> Result<&'a T, String> {
    option.as_ref().ok_or(format!("{name} is needed"))
}

/// `keys DIR`: new keys for the four roles, and the root they make.
fn keys(options: &Options) -> Result<(), String> {
    let [dir] = options.operands.as_slice() else {
        return Err("keys takes the folder to make them in".into());
    };
    let now = tuf::time::now();
    Keys::create(Path::new(dir), now)?;
    println!(
        "system-release: made keys for root, targets, snapshot and timestamp in {dir}, and root.json"
    );
    Ok(())
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

/// `check REPOSITORY`: the release there, read from the trusted root
/// offline, and every one of its files.
fn check(options: &Options) -> Result<(), String> {
    let [dir] = options.operands.as_slice() else {
        return Err(
            "check takes the folder of a release's repository, such as an install stick's release"
                .into(),
        );
    };
    let root_path = options.root.clone().unwrap_or_else(|| PathBuf::from(TRUSTED_ROOT));
    let root = std::fs::read(&root_path)
        .map_err(|err| format!("the trusted root {}: {err}", root_path.display()))?;
    let dir = Path::new(dir);
    let opened = repo::open(dir, &root, options.version.as_deref())?;
    let release = &opened.release;
    println!(
        "system-release: release {} ({}), built {} from commit {}, is signed by the trusted root",
        release.version,
        &opened.name[..16],
        release.built,
        release.commit
    );
    match repo::check_files(dir, release) {
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
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
