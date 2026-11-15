// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob fixture release VERSION`: the disk of an installed system of an
//! old release, for the boot tests that update from it (ADR 0536).
//!
//! The disk is made by that release's own tree: the tag `vVERSION` is
//! checked out in a git worktree under `target/fixtures/VERSION/src`, and
//! its own `./ob image --installed --user alice --password-file FILE`
//! builds it, as it built the release's disk then. The release trusts the
//! root of the update keys that sign what it is offered, so the keys of
//! this checkout (`.cache/release-keys`, made first if there are none) are
//! copied into the old tree before it builds, and a copy of them is kept
//! with the disk, in `target/fixtures/VERSION/release-keys`: a machine
//! that boots the disk only accepts an update signed by those keys.
//!
//! The boot tests of the `update-from-VERSION` cases boot a copy-on-write
//! overlay of `installed.img`, which is never written. A run that boots
//! one finds the keys by [`adopt_keys`]: before anything is signed, the
//! fixture's keys become this checkout's when it has none, and two sets
//! that differ stop the run.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result};

/// The password of alice, the account the fixture's disk is made with.
pub const PASSWORD: &str = "queen of hearts";

/// The account's full name.
pub const FULL_NAME: &str = "Alice Liddell";

/// The folder, in `target`, that holds a fixture for each release.
const FIXTURES: &str = "fixtures";

/// The fixture's disk, in its folder.
const IMAGE: &str = "installed.img";

/// The copy of the keys the disk trusts, in the fixture's folder.
const KEYS: &str = "release-keys";

/// The old tree's checkout, in the fixture's folder.
const SOURCE: &str = "src";

/// The file the disk's password is read from while it is made.
const PASSWORD_FILE: &str = "password";

/// What the old tree's build keeps and this checkout's can seed it with,
/// so that only what the old release changed is built again: the built
/// ports and the sources they were built from, GRUB and memtest86+.
const SEEDS: [&str; 6] = [
    "out/sysroot",
    "out/toolchain",
    "out/ports",
    "target/ports/sources",
    ".cache/grub",
    ".cache/memtest",
];

/// The folder of the fixture of `version`, in the Cargo output `target`.
pub fn dir(target: &Path, version: &str) -> PathBuf {
    target.join(FIXTURES).join(version)
}

/// The fixture's disk.
pub fn image(target: &Path, version: &str) -> PathBuf {
    dir(target, version).join(IMAGE)
}

/// The copy of the keys the fixture's disk trusts.
pub fn keys(target: &Path, version: &str) -> PathBuf {
    dir(target, version).join(KEYS)
}

/// The command that makes the fixture of `version`, as a message names it.
pub fn make_command(version: &str) -> String {
    format!("./ob fixture release {version}")
}

/// Whether `version` is a release's version as the tag `v`*version* names
/// it: numbers, with a pre-release or build part after a `-` or `+`. It
/// becomes a folder's name and a git tag, so nothing else passes.
pub fn valid_version(version: &str) -> bool {
    let (core, rest) = match version.find(['-', '+']) {
        Some(at) => (&version[..at], Some(&version[at + 1..])),
        None => (version, None),
    };
    let numbers: Vec<&str> = core.split('.').collect();
    let number = |n: &&str| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
    numbers.len() == 3
        && numbers.iter().all(number)
        && rest.is_none_or(|rest| {
            !rest.is_empty()
                && rest.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        })
}

/// `ob fixture release`: makes the fixture of `version`, or with `check`
/// says whether it is there and trusts this checkout's keys.
pub fn command(ctx: &Ctx, version: Option<String>, check: bool) -> Result<()> {
    let Some(version) = version else {
        crate::bail!("name the release to make a fixture of: ./ob fixture release 0.15.0");
    };
    if !valid_version(&version) {
        crate::bail!("`{version}` is no release's version: give it as 0.15.0, without the `v`");
    }
    let local = ctx.root.join(crate::release::KEYS_DIR);
    if check {
        let state = state(&ctx.target, &version, &local);
        println!("{}", state.describe(&version));
        return if state.usable() { Ok(()) } else { Err(Error::new(state.describe(&version))) };
    }
    make(ctx, &version, &local)
}

/// What a fixture is like next to this checkout's keys.
#[derive(Debug, PartialEq, Eq)]
pub enum State {
    /// There is no disk, or no copy of its keys.
    Missing,
    /// The disk trusts the keys of this checkout.
    Trusts,
    /// This checkout has no keys yet: a boot test adopts the fixture's.
    Adoptable,
    /// The disk trusts keys other than this checkout's.
    Differs,
}

impl State {
    /// Whether a boot test can use the fixture as it is.
    pub fn usable(&self) -> bool {
        matches!(self, State::Trusts | State::Adoptable)
    }

    /// The state in a sentence, which says what to do when it is not
    /// usable.
    pub fn describe(&self, version: &str) -> String {
        match self {
            State::Missing => {
                format!("there is no fixture of {version}; make it with {}", make_command(version))
            }
            State::Trusts => {
                format!("the fixture of {version} is there and trusts {}", crate::release::KEYS_DIR)
            }
            State::Adoptable => format!(
                "the fixture of {version} is there; {} has no keys, so a boot test uses the \
                 fixture's",
                crate::release::KEYS_DIR
            ),
            State::Differs => format!(
                "the fixture of {version} trusts keys other than those in {}: remove {} to use \
                 the fixture's, or remake the fixture with {}",
                crate::release::KEYS_DIR,
                crate::release::KEYS_DIR,
                make_command(version)
            ),
        }
    }
}

/// The state of the fixture of `version` in `target` next to the keys in
/// `local`.
pub fn state(target: &Path, version: &str, local: &Path) -> State {
    let kept = keys(target, version);
    if !image(target, version).is_file() || !has_keys(&kept) {
        return State::Missing;
    }
    if !has_keys(local) {
        return State::Adoptable;
    }
    if same_files(&kept, local) { State::Trusts } else { State::Differs }
}

/// Whether `dir` holds a set of release keys: the root `release::keys`
/// looks for.
fn has_keys(dir: &Path) -> bool {
    dir.join("root.json").is_file()
}

/// Whether `a` and `b` hold the same files, by their names and bytes.
fn same_files(a: &Path, b: &Path) -> bool {
    match (files(a), files(b)) {
        (Ok(a_files), Ok(b_files)) => {
            a_files == b_files
                && a_files.iter().all(|name| {
                    std::fs::read(a.join(name)).ok() == std::fs::read(b.join(name)).ok()
                })
        }
        _ => false,
    }
}

/// The files under `dir`, as paths relative to it, sorted.
fn files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    fn walk(dir: &Path, at: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir.join(at))? {
            let entry = entry?;
            let path = at.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                walk(dir, &path, out)?;
            } else {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, Path::new(""), &mut out)?;
    out.sort();
    Ok(out)
}

/// Copies the folder `from` to `to`, files and folders, with their modes.
pub fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// What [`adopt_keys`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Adopted {
    /// The checkout's keys are the fixture's already.
    Same,
    /// The checkout had no keys: the fixture's were copied in.
    Copied,
}

/// Makes the keys in `local` those the fixture of `version` in `target`
/// trusts, before anything is signed with them: copies them in when there
/// are none, and stops when there are others. The error names what to
/// remove, or the command that makes a fixture that is missing.
pub fn adopt_keys(target: &Path, version: &str, local: &Path) -> Result<Adopted> {
    match state(target, version, local) {
        State::Missing => crate::bail!(
            "the case update-from-{version} boots the fixture of {version}, which is not made: \
             run {}",
            make_command(version)
        ),
        State::Trusts => Ok(Adopted::Same),
        State::Adoptable => {
            let kept = keys(target, version);
            copy_dir(&kept, local)
                .context(|| format!("copying {} to {}", kept.display(), local.display()))?;
            Ok(Adopted::Copied)
        }
        State::Differs => crate::bail!(
            "the release keys in {} are not those the fixture of {version} trusts, in {}: remove \
             {} to use the fixture's, or remove {} and make the fixture again with {}",
            local.display(),
            keys(target, version).display(),
            local.display(),
            dir(target, version).display(),
            make_command(version)
        ),
    }
}

/// The arguments of the old tree's `./ob image` that make the disk: an
/// installed system with alice, an administrator, whose password is read
/// from `password_file` and hashed at the boot tests' cost, which is
/// quick to check under emulation.
pub fn image_args(password_file: &Path) -> Vec<String> {
    [
        "image",
        "--installed",
        "--user",
        "alice",
        "--full-name",
        FULL_NAME,
        "--password-file",
        &password_file.display().to_string(),
        "--test-cost",
    ]
    .map(String::from)
    .to_vec()
}

/// Where the old tree's `./ob image --installed` leaves its disk.
pub fn built_image(source: &Path, version: &str) -> PathBuf {
    source.join("out").join(format!("dukovo-{version}-x86_64-installed.img"))
}

/// `git` in the repository `root`.
fn git(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root);
    cmd
}

/// Fetches the tag of `version`, unless this repository has it.
fn fetch_tag(ctx: &Ctx, version: &str) -> Result<()> {
    let tag = format!("v{version}");
    let mut have = git(&ctx.root);
    have.args(["rev-parse", "--verify", "--quiet"]).arg(format!("refs/tags/{tag}^{{commit}}"));
    if process::output(ctx, &mut have).is_ok() {
        return Ok(());
    }
    step(&format!("Fetching the tag {tag}"));
    let mut fetch = git(&ctx.root);
    fetch.args(["fetch", "origin"]).arg(format!("refs/tags/{tag}:refs/tags/{tag}"));
    process::run(ctx, &mut fetch)
}

/// Removes the worktree at `source`, if it is one, and what is left of it.
fn remove_worktree(ctx: &Ctx, source: &Path) {
    let mut remove = git(&ctx.root);
    remove.args(["worktree", "remove", "--force"]).arg(source);
    let _ = process::output(ctx, &mut remove);
    let _ = std::fs::remove_dir_all(source);
    let mut prune = git(&ctx.root);
    prune.args(["worktree", "prune"]);
    let _ = process::output(ctx, &mut prune);
}

/// Makes the fixture of `version`.
fn make(ctx: &Ctx, version: &str, local: &Path) -> Result<()> {
    crate::release::keys(ctx)?;
    if state(&ctx.target, version, local) == State::Trusts {
        step(&format!(
            "The fixture of {version} is made already and trusts {}",
            crate::release::KEYS_DIR
        ));
        return Ok(());
    }
    fetch_tag(ctx, version)?;
    let folder = dir(&ctx.target, version);
    let source = folder.join(SOURCE);
    remove_worktree(ctx, &source);
    if folder.exists() {
        std::fs::remove_dir_all(&folder).context(|| format!("removing {}", folder.display()))?;
    }
    process::create_dir(&folder)?;
    let built = build(ctx, version, local, &folder, &source);
    remove_worktree(ctx, &source);
    built?;
    println!("\nFixture ready: {}", ctx.display(&image(&ctx.target, version)));
    Ok(())
}

/// Checks the tag out at `source` and builds the disk there, keeping it
/// and a copy of the keys in `folder`.
fn build(ctx: &Ctx, version: &str, local: &Path, folder: &Path, source: &Path) -> Result<()> {
    step(&format!("Checking out v{version} in {}", ctx.display(source)));
    let mut add = git(&ctx.root);
    add.args(["worktree", "add", "--detach"]).arg(source).arg(format!("v{version}"));
    process::run(ctx, &mut add)?;
    // The old release trusts the root that signs what it is offered.
    let keys_in = source.join(crate::release::KEYS_DIR);
    copy_dir(local, &keys_in)
        .context(|| format!("copying {} to {}", local.display(), keys_in.display()))?;
    for seed in SEEDS {
        let from = ctx.root.join(seed);
        if from.exists() {
            seed_from(ctx, &from, &source.join(seed))?;
        }
    }
    let password = folder.join(PASSWORD_FILE);
    std::fs::write(&password, format!("{PASSWORD}\n"))
        .context(|| format!("writing {}", password.display()))?;
    step(&format!("Building the installed disk of {version} with its own ./ob"));
    let mut cmd = Command::new(source.join("ob"));
    cmd.current_dir(source).args(image_args(&password));
    for variable in ["CARGO_TARGET_DIR", "OB_TARGET_DIR", "OB_ROOT"] {
        cmd.env_remove(variable);
    }
    let ran = process::run(ctx, &mut cmd);
    let _ = std::fs::remove_file(&password);
    ran?;
    let built = built_image(source, version);
    if !built.is_file() {
        crate::bail!("the old tree's ./ob image left no disk at {}", built.display());
    }
    // The disk is the last to appear: a fixture is there once it is.
    let kept = keys(&ctx.target, version);
    copy_dir(local, &kept)
        .context(|| format!("copying {} to {}", local.display(), kept.display()))?;
    let to = image(&ctx.target, version);
    std::fs::rename(&built, &to)
        .context(|| format!("moving {} to {}", built.display(), to.display()))
}

/// Copies `from` to `to` as the old tree's own, sharing the data where
/// the filesystem allows it.
fn seed_from(ctx: &Ctx, from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        process::create_dir(parent)?;
    }
    let mut cp = Command::new("cp");
    cp.args(["-a", "--reflink=auto"]).arg(from).arg(to);
    process::run(ctx, &mut cp)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of this test's own in the temporary folder, empty.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ob-fixture-release-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a set of keys with `root` as its root into `dir`.
    fn write_keys(dir: &Path, root: &str) {
        std::fs::create_dir_all(dir.join("online")).unwrap();
        std::fs::write(dir.join("root.json"), root).unwrap();
        std::fs::write(dir.join("online/key"), "secret").unwrap();
    }

    /// A fixture of 0.15.0 in `target`, trusting a set with `root`.
    fn write_fixture(target: &Path, root: &str) {
        std::fs::create_dir_all(dir(target, "0.15.0")).unwrap();
        std::fs::write(image(target, "0.15.0"), "disk").unwrap();
        write_keys(&keys(target, "0.15.0"), root);
    }

    #[test]
    fn the_fixture_is_kept_in_the_targets_fixtures_folder_by_version() {
        let target = Path::new("/work/target");
        assert_eq!(dir(target, "0.15.0"), Path::new("/work/target/fixtures/0.15.0"));
        assert_eq!(
            image(target, "0.15.0"),
            Path::new("/work/target/fixtures/0.15.0/installed.img")
        );
        assert_eq!(keys(target, "0.15.0"), Path::new("/work/target/fixtures/0.15.0/release-keys"));
        assert_eq!(make_command("0.15.0"), "./ob fixture release 0.15.0");
    }

    #[test]
    fn only_a_releases_version_names_a_fixture() {
        for good in ["0.15.0", "1.20.300", "0.16.0-rc.1", "0.16.0+build-5"] {
            assert!(valid_version(good), "{good}");
        }
        for bad in [
            "",
            "0.15",
            "v0.15.0",
            "0.15.0.1",
            "../0.15.0",
            "0.15.0/x",
            "0.15.",
            "a.b.c",
            "0.15.0-",
            "0.15.0 x",
        ] {
            assert!(!valid_version(bad), "{bad}");
        }
    }

    #[test]
    fn the_old_trees_image_is_made_for_alice_with_a_password_file() {
        let args = image_args(Path::new("/f/password"));
        assert_eq!(args[..3], ["image", "--installed", "--user"]);
        let at = args.iter().position(|a| a == "--password-file").unwrap();
        assert_eq!(args[at + 1], "/f/password");
        assert!(args.contains(&"alice".to_string()));
        assert!(args.contains(&"--test-cost".to_string()));
        assert_eq!(
            built_image(Path::new("/f/src"), "0.15.0"),
            Path::new("/f/src/out/dukovo-0.15.0-x86_64-installed.img")
        );
    }

    #[test]
    fn a_fixture_is_missing_until_its_disk_and_its_keys_are_there() {
        let target = scratch("missing");
        let local = target.join("local");
        assert_eq!(state(&target, "0.15.0", &local), State::Missing);
        std::fs::create_dir_all(dir(&target, "0.15.0")).unwrap();
        std::fs::write(image(&target, "0.15.0"), "disk").unwrap();
        assert_eq!(state(&target, "0.15.0", &local), State::Missing);
        write_keys(&keys(&target, "0.15.0"), "one");
        assert_eq!(state(&target, "0.15.0", &local), State::Adoptable);
        assert!(!State::Missing.usable());
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn a_fixture_trusts_the_keys_that_are_the_same_files() {
        let target = scratch("trusts");
        let local = target.join("local");
        write_fixture(&target, "one");
        write_keys(&local, "one");
        assert_eq!(state(&target, "0.15.0", &local), State::Trusts);
        // The same root with another file under it is another set.
        std::fs::write(local.join("online/key"), "other").unwrap();
        assert_eq!(state(&target, "0.15.0", &local), State::Differs);
        std::fs::write(local.join("online/key"), "secret").unwrap();
        std::fs::write(local.join("extra"), "x").unwrap();
        assert_eq!(state(&target, "0.15.0", &local), State::Differs);
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn keys_the_checkout_lacks_are_adopted_from_the_fixture() {
        let target = scratch("adopt");
        let local = target.join("cache/release-keys");
        write_fixture(&target, "one");
        assert_eq!(adopt_keys(&target, "0.15.0", &local).unwrap(), Adopted::Copied);
        assert_eq!(std::fs::read_to_string(local.join("root.json")).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(local.join("online/key")).unwrap(), "secret");
        assert_eq!(adopt_keys(&target, "0.15.0", &local).unwrap(), Adopted::Same);
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn a_folder_of_keys_without_a_root_is_no_keys() {
        let target = scratch("empty");
        let local = target.join("local");
        std::fs::create_dir_all(&local).unwrap();
        write_fixture(&target, "one");
        assert_eq!(adopt_keys(&target, "0.15.0", &local).unwrap(), Adopted::Copied);
        assert!(local.join("root.json").is_file());
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn other_keys_stop_the_run_and_say_what_to_remove() {
        let target = scratch("differs");
        let local = target.join("local");
        write_fixture(&target, "one");
        write_keys(&local, "two");
        let why = adopt_keys(&target, "0.15.0", &local).unwrap_err().to_string();
        assert!(why.contains(&format!("remove {}", local.display())), "{why}");
        assert!(why.contains("fixtures/0.15.0"), "{why}");
        assert!(why.contains("./ob fixture release 0.15.0"), "{why}");
        // Nothing was changed.
        assert_eq!(std::fs::read_to_string(local.join("root.json")).unwrap(), "two");
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn a_fixture_that_is_not_made_is_not_made_by_the_run() {
        let target = scratch("none");
        let local = target.join("local");
        let why = adopt_keys(&target, "0.15.0", &local).unwrap_err().to_string();
        assert!(why.contains("run ./ob fixture release 0.15.0"), "{why}");
        assert!(why.contains("update-from-0.15.0"), "{why}");
        assert!(!local.exists());
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn each_state_says_what_to_do() {
        assert!(State::Missing.describe("0.15.0").contains("./ob fixture release 0.15.0"));
        assert!(State::Differs.describe("0.15.0").contains("remove .cache/release-keys"));
        assert!(State::Trusts.usable() && State::Adoptable.usable() && !State::Differs.usable());
    }
}
