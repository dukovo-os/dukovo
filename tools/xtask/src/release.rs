// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Signed system releases, `ob`'s side (ADR 0308): the keys that sign
//! them, the root the boot archive carries so that a system trusts them,
//! and the release of a staged slot, made and signed by system-release,
//! which ob builds for this machine as it builds catalog-repo. `ob
//! release` deploys an update repository to the update server and checks
//! what it serves (ADR 0467).
//!
//! `ob release sign` signs the releases the update server offers on the
//! project's YubiKeys ([`crate::release_sign`], ADR 0778), and `ob
//! release images` makes the project's install stick and ISO of one,
//! which carry it as the server serves it and trust the release root
//! ([`crate::release_images`], ADR 0779). Any other image's release is
//! signed by the machine that builds it, with keys of its own, made the
//! first time they are needed in `.cache/release-keys`: such a stick
//! verifies against the root built into it, and nothing online trusts
//! that root. When a stick's release will not check, run `system-release
//! check` on the stick's `release` folder with `--root
//! .cache/release-keys/root.json`, or with `--root
//! releases/metadata/1.root.json` for the project's.

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

use crate::config::Profile;
use crate::deploy::{self, Server};
use crate::identity::hex;
use crate::process::{self, step};
use crate::{Context, Ctx, Result, VERSION, bail, image, kernel, userspace};

/// Where this machine's release keys are kept.
pub const KEYS_DIR: &str = ".cache/release-keys";

/// Where the boot archive carries the root a system trusts releases by,
/// which system-release(8) checks against by default.
pub const TRUSTED_ROOT: &str = "system/resources/updates/root.json";

/// Where a release's repository is on the install stick's DUKOVO_LIVE
/// partition: its `release` subvolume, read-only.
pub const SUBVOLUME: &str = "release";

/// The production repository's public half in the source tree (ADR
/// 0467): `keys/online.json`, the update server's online key, which `ob
/// release server setup` writes, and `metadata/1.root.json`, the first
/// root, which the key ceremony signs on the hardware keys (ADR 0457).
pub const REPO: &str = "releases";

/// Where the update server serves the system's releases.
pub const URL: &str = "https://updates.dukovo.org/system/";

/// The server's folder a deploy copies into (tools/update-server/): the
/// same on an Ubuntu server and a Dukovo one, where `/var` is the
/// system's state.
const INCOMING: &str = "/var/lib/dukovo-system-incoming";

/// The folders of files the server serves, whose names a deploy skips: an
/// Ubuntu server's and a Dukovo server's.
const SERVED_FILES: [&str; 2] =
    ["/srv/dukovo-system/system/files", "/var/lib/dukovo-system-site/system/files"];

/// The online key's public half, as the server's setup leaves it: an
/// Ubuntu server's and a Dukovo server's.
pub const SERVER_ONLINE_KEYS: [&str; 2] =
    ["/usr/local/share/dukovo-system/online.json", "/var/lib/dukovo-system/online.json"];

/// The script that signs the rollouts again, on an Ubuntu server and on a
/// Dukovo one.
const ROLLOUT_SCRIPTS: [&str; 2] = [
    "/usr/local/lib/dukovo-system/system-rollout.sh",
    "/var/lib/dukovo-update-server/system-rollout",
];

/// The script that changes the canaries and shows them, on an Ubuntu
/// server and on a Dukovo one (ADR 0533).
const CANARY_SCRIPTS: [&str; 2] =
    ["/usr/local/lib/dukovo-system/canaries.sh", "/var/lib/dukovo-update-server/canaries"];

/// What `rollout canary NAME red|green` is answered with, here and for the
/// catalog's `rollout`: the canaries are one list for both.
pub const CANARY_MOVED: &str = "`rollout canary NAME red|green` is gone: the canaries are one list \
     that the system's releases and the catalog both follow, changed with `./ob release canary \
     NAME red REASON`, `NAME green REASON` or `NAME auto`, and `./ob release canary add NAME KEY` \
     and `remove NAME` enrol and drop one; `./ob release canaries` shows them";

/// How long a deploy waits for the server to publish: it hashes every
/// new file as it takes it, and a Dukovo server starts its publish job
/// once a minute.
const PUBLISH_WAIT_SECONDS: u32 = 600;

/// Builds system-release for this machine and gives its path.
pub(crate) fn tool(ctx: &Ctx) -> Result<PathBuf> {
    tool_for(ctx, false)
}

/// Builds system-release for this machine, optimised for the update
/// server's when `release`, and gives its path.
pub fn tool_for(ctx: &Ctx, release: bool) -> Result<PathBuf> {
    step("Building system-release for this machine");
    let mut cmd = crate::userspace::host_cargo_of(ctx, "build", &["system-release"]);
    cmd.args(["--bin", "system-release"]);
    if release {
        cmd.arg("--release");
    }
    process::run(ctx, &mut cmd)?;
    let profile = if release { "release" } else { "debug" };
    Ok(ctx.target.join("programs-host").join(profile).join("system-release"))
}

/// `ob release`: sets up the update server, deploys an update repository
/// to it, or checks what it serves. `send` is `deploy` for a server that is
/// not the project's, a test's or a lab's: any first root, and no read-back
/// from updates.dukovo.org; with `--copy-only` it stops before it says the
/// deploy is ready, which the lab's test of the move shows is never published.
pub fn command(ctx: &Ctx, args: &[String]) -> Result<()> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["build", rest @ ..] => build(ctx, rest),
        ["sign", rest @ ..] => crate::release_sign::sign(ctx, rest),
        ["record", version] => crate::release_sign::record(ctx, version),
        ["images", rest @ ..] => crate::release_images::images(ctx, rest),
        ["key", "yubikey"] => release_key(ctx, None),
        ["key", "yubikey", "--serial", serial] => release_key(ctx, Some(serial)),
        ["root"] => first_root(ctx),
        ["server", "setup"] => crate::catalog::server_setup(ctx, false),
        ["server", "setup", "--dukovo"] => crate::catalog::server_setup(ctx, true),
        ["channel", repo, "--firmware", set, channel] => {
            channel_command(ctx, Path::new(repo), &["--firmware", set, channel])
        }
        ["channel", repo, channel, version] => {
            channel_command(ctx, Path::new(repo), &[channel, version])
        }
        ["advisories", repo] => advisories(ctx, Path::new(repo)),
        ["deploy", dir] => deploy(ctx, Path::new(dir)),
        ["send", dir] => publish_to(ctx, &Server::from_environment(), Path::new(dir)),
        ["send", "--copy-only", dir] => copy_to(ctx, &Server::from_environment(), Path::new(dir)),
        ["check"] => check(ctx, None, URL),
        ["check", url] => check(ctx, None, url),
        ["check", "--root", root, url] => check(ctx, Some(Path::new(root)), url),
        ["rollout", "canary", ..] => bail!("{CANARY_MOVED}"),
        ["rollout", rest @ ..] if !rest.is_empty() => rollout(ctx, rest),
        ["canaries"] => canaries(ctx, false),
        ["canaries", "--json"] => canaries(ctx, true),
        ["canary", rest @ ..] if !rest.is_empty() => canary(ctx, rest),
        _ => bail!(
            "usage: ./ob release build [--out <dir>] \
             | sign <version> [--channel <channel>] [--serial <number>] | record <version> \
             | images <version> [--serial <number>] \
             | key yubikey [--serial <number>] | root \
             | server setup [--dukovo] | channel <repository> (<channel> \
             <version> | --firmware <version>|none <channel>) | advisories <repository> \
             | deploy <repository> | send [--copy-only] <repository> \
             | check [--root <file>] [<url>] \
             | rollout (pause <version> <reason> | resume <version>) | canaries [--json] \
             | canary (add <name> <key> | remove <name> | <name> red|green <reason> | <name> auto)"
        ),
    }
}

/// `ob release key yubikey [--serial N]`: keeps the public half of a
/// YubiKey's release key, in its slot 95, in `releases/keys/` (ADR 0538).
fn release_key(ctx: &Ctx, serial: Option<&str>) -> Result<()> {
    let mut cmd = Command::new(tool(ctx)?);
    cmd.arg("release-key")
        .arg("--yubikey-helper")
        .arg(ctx.root.join(crate::catalog::YUBIKEY_HELPER));
    if let Some(serial) = serial {
        cmd.args(["--serial", serial]);
    }
    cmd.arg(ctx.root.join(REPO).join("keys"));
    process::run(ctx, &mut cmd)
}

/// `ob release root`: the key ceremony's first root of the system's
/// releases, signed on both YubiKeys from the keys in `releases/keys/`,
/// in `releases/metadata/1.root.json` (ADRs 0457 and 0538).
fn first_root(ctx: &Ctx) -> Result<()> {
    let repo = ctx.root.join(REPO);
    let mut cmd = Command::new(tool(ctx)?);
    cmd.arg("first-root")
        .arg("--yubikey-helper")
        .arg(ctx.root.join(crate::catalog::YUBIKEY_HELPER))
        .arg("--out")
        .arg(repo.join("metadata"))
        .arg(repo.join("keys"));
    process::run(ctx, &mut cmd)
}

/// `ob release channel REPOSITORY ...`: has the channel of the update
/// repository offer a release, or a firmware set, with the security
/// advisories of the source tree signed in the same targets version (ADR
/// 0534). `words` are what system-release's `channel` takes after the
/// repository and keys.
fn channel_command(ctx: &Ctx, repo: &Path, words: &[&str]) -> Result<()> {
    let mut args = repository_args("channel", ctx, repo)?;
    args.extend(["--advisories".into(), ctx.root.join(crate::security::DIR).into_os_string()]);
    args.extend(words.iter().map(std::ffi::OsString::from));
    system_release(ctx, &args)
}

/// `ob release advisories REPOSITORY`: signs the security advisories of
/// the source tree into the update repository, with the index, in one
/// targets version, as a channel does; it signs nothing when they are as
/// the repository has them. This is how a corrected advisory goes out
/// without a release (ADR 0534).
fn advisories(ctx: &Ctx, repo: &Path) -> Result<()> {
    let mut args = repository_args("advisories", ctx, repo)?;
    args.push(ctx.root.join(crate::security::DIR).into_os_string());
    system_release(ctx, &args)
}

/// `command --repo REPOSITORY --keys KEYS`, for system-release, with this
/// machine's release keys.
fn repository_args(command: &str, ctx: &Ctx, repo: &Path) -> Result<Vec<std::ffi::OsString>> {
    Ok(vec![
        command.into(),
        "--repo".into(),
        repo.as_os_str().to_os_string(),
        "--keys".into(),
        keys(ctx)?.into_os_string(),
    ])
}

/// `ob release rollout ...`: on the update server, as its user (through
/// sudo, or through elevate on a Dukovo server), pauses or resumes a
/// release's waves and signs the rollouts again (ADR 0469); needs no
/// release key.
fn rollout(ctx: &Ctx, args: &[&str]) -> Result<()> {
    let words: Vec<String> = args.iter().map(|word| deploy::quote(word)).collect();
    let [ubuntu, dukovo] = ROLLOUT_SCRIPTS;
    let words = words.join(" ");
    let script = format!(
        "if [ -x {ubuntu} ]; then sudo -u dukovo-system {ubuntu} {words}; \
         else elevate sh {dukovo} {words}; fi"
    );
    // A terminal, for sudo or elevate to ask for the password.
    process::run(ctx, &mut Server::from_environment().ssh_terminal(&script))
}

/// What `ob release canary ...` is told, as the words of the server's
/// canaries script: `add NAME KEY`, `remove NAME`, `NAME red REASON`,
/// `NAME green REASON` or `NAME auto`.
fn canary_words_are_valid(words: &[&str]) -> bool {
    match words {
        ["add", _, _] | ["remove", _] | [_, "auto"] => true,
        [_, "red" | "green", reason @ ..] => !reason.is_empty(),
        _ => false,
    }
}

/// `ob release canary ...`: on the update server, as root (through sudo,
/// or through elevate on a Dukovo server), a canary enrolled with the key
/// `sysupdate canary enrol` printed, taken off the list, or its colour set
/// by hand, which wins over its reports until `auto` (ADR 0533). The jobs
/// that hold the waves see the change at once, or within five minutes on a
/// Dukovo server.
fn canary(ctx: &Ctx, words: &[&str]) -> Result<()> {
    if !canary_words_are_valid(words) {
        bail!(
            "usage: ./ob release canary add <name> <key> | remove <name> | <name> red <reason> \
             | <name> green <reason> | <name> auto"
        );
    }
    let words: Vec<String> = words.iter().map(|word| deploy::quote(word)).collect();
    let [ubuntu, dukovo] = CANARY_SCRIPTS;
    let words = words.join(" ");
    let script = format!(
        "if [ -x {ubuntu} ]; then sudo {ubuntu} {words}; else elevate sh {dukovo} {words}; fi"
    );
    // A terminal, for sudo or elevate to ask for the password.
    process::run(ctx, &mut Server::from_environment().ssh_terminal(&script))
}

/// `ob release canaries [--json]`: what the canaries make of the waves,
/// read on the update server with no terminal and no password: each
/// canary's colour and why, when it last reported and what it runs. On an
/// Ubuntu server it runs through a sudo rule that setup.sh writes for
/// this one command (ADR 0533).
fn canaries(ctx: &Ctx, json: bool) -> Result<()> {
    let [ubuntu, dukovo] = CANARY_SCRIPTS;
    let flag = if json { " --json" } else { "" };
    let script = format!(
        "if [ -x {ubuntu} ]; then sudo -n {ubuntu} show{flag}; else sh {dukovo} show{flag}; fi"
    );
    let output =
        process::output(ctx, &mut Server::from_environment().ssh(&script)).map_err(|err| {
            err.hint("a server set up before the canaries needs ./ob release server setup again")
        })?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

/// The production first root, which every repository deployed must start
/// from.
pub(crate) fn production_root(ctx: &Ctx) -> Result<Vec<u8>> {
    let path = ctx.root.join(REPO).join("metadata/1.root.json");
    std::fs::read(&path).map_err(|_| {
        crate::Error::new(format!(
            "{REPO}/metadata/1.root.json is not there yet: the key ceremony signs the first root \
             of the system's releases on the hardware keys (ADR 0457), and until then nothing \
             is deployed"
        ))
    })
}

/// `ob release deploy DIR`: copies the update repository at `dir`, signed
/// by the release keys from the production root, to the server, which
/// publishes it with its online key and says whether that worked; then
/// reads it back over https.
fn deploy(ctx: &Ctx, dir: &Path) -> Result<()> {
    let root = production_root(ctx)?;
    let first = dir.join("metadata/1.root.json");
    match std::fs::read(&first) {
        Ok(bytes) if bytes == root => {}
        Ok(_) => bail!(
            "{}'s first root is not {REPO}/metadata/1.root.json: it was not signed by the release \
             keys, and is not deployed",
            dir.display()
        ),
        Err(_) => bail!("{} is not an update repository: it has no first root", dir.display()),
    }
    publish_to(ctx, &Server::from_environment(), dir)?;
    check(ctx, None, URL)
}

/// Copies the update repository at `dir` to `server` and has it publish
/// it, whatever root it starts from: [`deploy`] checks that, and the boot
/// tests deploy repositories of roots of their own.
///
/// # Errors
///
/// If the server does not take the repository or could not publish it.
pub fn publish_to(ctx: &Ctx, server: &Server, dir: &Path) -> Result<()> {
    copy_to(ctx, server, dir)?;
    deploy::publish(
        ctx,
        server,
        INCOMING,
        PUBLISH_WAIT_SECONDS,
        "system releases",
        "dukovo-system-publish",
    )
}

/// Copies the update repository at `dir` into the server's incoming
/// folder, which [`publish_to`] does before it says the deploy is ready:
/// the boot tests leave a deploy there, to see that the server publishes
/// nothing of it.
///
/// # Errors
///
/// If the server does not take the repository.
pub fn copy_to(ctx: &Ctx, server: &Server, dir: &Path) -> Result<()> {
    step(&format!("Copying the repository to {}", server.dest()));
    deploy::replace(ctx, server, dir, &["metadata", "targets"], INCOMING, INCOMING)?;
    // Files are named by their SHA-256, so one the server holds is not
    // sent again, and none is ever taken away from it.
    deploy::add_missing(
        ctx,
        server,
        &dir.join("files"),
        deploy::is_sha256_name,
        INCOMING,
        &format!("{INCOMING}/files"),
        &SERVED_FILES,
    )
}

/// `ob release check`: reads the repository `url` serves as a computer
/// online would, from the production first root, or from the first root in
/// `root` for a server of a test or a lab: every release list and channel.
fn check(ctx: &Ctx, root: Option<&Path>, url: &str) -> Result<()> {
    let root = match root {
        Some(root) => root.to_path_buf(),
        None => {
            production_root(ctx)?;
            ctx.root.join(REPO).join("metadata/1.root.json")
        }
    };
    let mut cmd = Command::new(tool(ctx)?);
    cmd.arg("served").arg("--root").arg(root).arg(url);
    process::run(ctx, &mut cmd)
}

/// This machine's release keys, made the first time they are needed.
pub fn keys(ctx: &Ctx) -> Result<PathBuf> {
    let dir = ctx.root.join(KEYS_DIR);
    if !dir.join("root.json").is_file() {
        step(&format!("Making this machine's release keys in {KEYS_DIR}"));
        let mut cmd = Command::new(tool(ctx)?);
        cmd.arg("keys").arg(&dir);
        process::run(ctx, &mut cmd)?;
    }
    Ok(dir)
}

/// The root a build of the system puts in the boot archive in place of
/// this machine's, when `./ob release build` has pinned it ([`pin_root`]).
static PINNED_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Has every build of the system in this run trust the root in the file
/// `root`, as `./ob release build` does: a tree that carried this
/// machine's own root would differ from the same tree built on another
/// machine, whose keys differ (ADR 0535). The first root given is kept.
pub fn pin_root(root: &Path) {
    let _ = PINNED_ROOT.set(root.to_path_buf());
}

/// The root a system trusts releases by, for the boot archive: its path
/// there, [`TRUSTED_ROOT`], and a copy of this machine's, readable by
/// all, or the one [`pin_root`] pinned.
pub fn trusted_root(ctx: &Ctx) -> Result<(String, PathBuf)> {
    let from = match PINNED_ROOT.get() {
        Some(root) => root.clone(),
        None => keys(ctx)?.join("root.json"),
    };
    let copy = ctx.out.join("release/root.json");
    process::copy(&from, &copy)?;
    std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o644))
        .context(|| format!("setting the mode of {}", copy.display()))?;
    Ok((TRUSTED_ROOT.to_string(), copy))
}

/// The root a release build trusts until the key ceremony signs the real
/// one: made once with `system-release keys`, its keys thrown away, so
/// that no one can sign for it. Nothing that trusts it is ever shipped.
pub const STAND_IN_ROOT: &str = "releases/stand-in-root.json";

/// The root `./ob release build` puts in the tree: the system's first
/// root, signed on the hardware keys, once it is in the repository
/// ([`REPO`]), and [`STAND_IN_ROOT`] until then.
fn build_root(ctx: &Ctx) -> PathBuf {
    let signed = ctx.root.join(REPO).join("metadata/1.root.json");
    if signed.is_file() { signed } else { ctx.root.join(STAND_IN_ROOT) }
}

/// The names of the files `./ob release build` writes into its output
/// folder: the tree's file list, the release list and its SHA-256.
pub const BUILD_FILES: [&str; 3] = ["files.txt", "system.json", "system.json.sha256"];

/// Where `./ob release build` stages the tree, below `out/`, and leaves
/// it: a file that differs between two builds is read from there.
pub(crate) const BUILD_TREE: &str = "release-build-tree";

/// `./ob release build [--out DIR]`: builds the system as this commit
/// makes it, stages its slot as an image and a release hold it, and
/// writes what two builds are compared by into `DIR`, by default
/// `out/release-build` (ADR 0535): [`file_list`] as `files.txt`, the
/// release list, unsigned, as `system.json`, and its SHA-256 as
/// `system.json.sha256`. The last line on the standard output is that
/// SHA-256, the value that ADR 0457's three builds must agree on. The tree
/// is left in `out/release-build-tree`, for reading a file that differs.
///
/// Nothing is signed, since a build machine's keys differ, and the tree
/// trusts [`build_root`], not this machine's keys, so that it is the tree
/// every machine makes.
fn build(ctx: &Ctx, args: &[&str]) -> Result<()> {
    let out = match args {
        [] => ctx.out.join("release-build"),
        ["--out", dir] => std::path::absolute(dir)
            .map_err(|err| crate::Error::new(format!("--out {dir}: {err}")))?,
        _ => bail!("usage: ./ob release build [--out <dir>]"),
    };
    let digest = build_release(ctx, &out)?;
    println!("{digest}");
    Ok(())
}

/// [`build`] into `out`, which gives the release list's SHA-256 and
/// leaves the tree in `out/`[`BUILD_TREE`]; `./ob release sign` builds
/// the third time with it.
pub(crate) fn build_release(ctx: &Ctx, out: &Path) -> Result<String> {
    if image::source_date_epoch(ctx).is_none() {
        bail!(
            "a release is built at the time of its commit: run this in a git checkout, or set \
             SOURCE_DATE_EPOCH"
        );
    }
    let root = build_root(ctx);
    if !root.is_file() {
        bail!("{} is not there: a release trusts a root", ctx.display(&root));
    }
    pin_root(&root);
    step(&format!("Building the release of this commit, trusting {}", ctx.display(&root)));
    let kernel = kernel::build(ctx, Profile::Release)?;
    let programs = userspace::build(ctx, Profile::Release)?;
    let tree = ctx.out.join(BUILD_TREE);
    build_in(ctx, &kernel, &programs, &tree, out)
}

/// The second half of [`build`]: stages the slot in `tree`, and writes the
/// files into `out`. Returns the release list's SHA-256.
fn build_in(
    ctx: &Ctx,
    kernel: &Path,
    programs: &userspace::Programs,
    tree: &Path,
    out: &Path,
) -> Result<String> {
    image::stage_system(ctx, kernel, programs, tree)?;
    step(&format!("Listing the release into {}", ctx.display(out)));
    if out.exists() {
        std::fs::remove_dir_all(out).context(|| format!("removing {}", out.display()))?;
    }
    process::create_dir(out)?;
    let [files, list, sum] = BUILD_FILES.map(|name| out.join(name));
    std::fs::write(&files, file_list(tree)?).context(|| format!("writing {}", files.display()))?;
    let mut cmd = Command::new(tool(ctx)?);
    cmd.arg("list")
        .arg("--tree")
        .arg(tree)
        .arg("--out")
        .arg(&list)
        .args(["--version", VERSION, "--updates-from", UPDATES_FROM])
        .args(["--commit", &commit(ctx)])
        .args(["--toolchain", &toolchain(ctx)?])
        .args(["--built", &image::image_time(ctx).to_string()]);
    process::run(ctx, &mut cmd)?;
    let bytes = std::fs::read(&list).context(|| format!("reading {}", list.display()))?;
    let digest = hex(&Sha256::digest(&bytes));
    std::fs::write(&sum, format!("{digest}  system.json\n"))
        .context(|| format!("writing {}", sum.display()))?;
    Ok(digest)
}

/// What a tree holds, as lines of text that two builds of one commit
/// compare with `diff`: one line for each folder, file and link under
/// `tree`, sorted by path as bytes, with its fields separated by tabs:
///
/// - its kind, `d`, `f` or `l`;
/// - its mode in octal, four digits;
/// - a file's SHA-256, and `-` for the others;
/// - a file's size in bytes, and `-`;
/// - a file's modification time in seconds since 1970, which an image
///   keeps and a release list does not, and `-`;
/// - its path below `tree`;
/// - and, for a link, its target.
///
/// # Errors
///
/// If something in the tree cannot be read, is neither a folder, a file
/// nor a link, or has a name that is not UTF-8.
pub fn file_list(tree: &Path) -> Result<String> {
    let mut lines: Vec<(String, String)> = Vec::new();
    let mut folders = vec![(tree.to_path_buf(), String::new())];
    while let Some((folder, at)) = folders.pop() {
        let entries =
            std::fs::read_dir(&folder).context(|| format!("reading {}", folder.display()))?;
        for entry in entries {
            let path = entry.context(|| format!("reading {}", folder.display()))?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| crate::Error::new(format!("{} is not UTF-8", path.display())))?;
            let shown = if at.is_empty() { name.to_string() } else { format!("{at}/{name}") };
            let meta = std::fs::symlink_metadata(&path)
                .context(|| format!("reading {}", path.display()))?;
            let mode = meta.permissions().mode() & 0o7777;
            let line = if meta.is_dir() {
                folders.push((path, shown.clone()));
                format!("d\t{mode:04o}\t-\t-\t-\t{shown}")
            } else if meta.is_file() {
                let mut file =
                    std::fs::File::open(&path).context(|| format!("reading {}", path.display()))?;
                let mut hasher = Sha256::new();
                std::io::copy(&mut file, &mut hasher)
                    .context(|| format!("reading {}", path.display()))?;
                let sum = hex(&hasher.finalize());
                format!("f\t{mode:04o}\t{sum}\t{}\t{}\t{shown}", meta.len(), meta.mtime())
            } else if meta.file_type().is_symlink() {
                let target =
                    std::fs::read_link(&path).context(|| format!("reading {}", path.display()))?;
                let target = target.to_str().ok_or_else(|| {
                    crate::Error::new(format!("the link {} is not UTF-8", path.display()))
                })?;
                format!("l\t{mode:04o}\t-\t-\t-\t{shown}\t{target}")
            } else {
                bail!("{} is neither a file, a folder nor a link", path.display());
            };
            lines.push((shown, line));
        }
    }
    lines.sort();
    let mut text = String::new();
    for (_, line) in lines {
        text.push_str(&line);
        text.push('\n');
    }
    Ok(text)
}

/// The oldest version a release updates from directly (the updates
/// design's section 8): the first with atomic updates, until a release
/// needs a bridge.
pub const UPDATES_FROM: &str = "0.14.1";

/// Makes and signs the release of the slot staged at `tree` into `out`, a
/// repository as the update server will serve one: the release list as a
/// TUF target, and every file under its SHA-256, read back before it is
/// done.
pub fn make(ctx: &Ctx, tree: &Path, out: &Path) -> Result<()> {
    make_as(ctx, tree, out, VERSION, UPDATES_FROM)
}

/// [`make`] with the release named `version`, updating from
/// `updates_from` on: the boot tests' updates are this build with a
/// version of their own.
pub fn make_as(
    ctx: &Ctx,
    tree: &Path,
    out: &Path,
    version: &str,
    updates_from: &str,
) -> Result<()> {
    make_with(ctx, &keys(ctx)?, tree, out, version, updates_from)
}

/// [`make_as`], signed with the keys in `keys`: the boot tests' update
/// server's, which start as this machine's and have had keys replaced.
pub fn make_with(
    ctx: &Ctx,
    keys: &Path,
    tree: &Path,
    out: &Path,
    version: &str,
    updates_from: &str,
) -> Result<()> {
    step(&format!("Signing release {version} into {}", ctx.display(out)));
    let mut cmd = Command::new(tool(ctx)?);
    cmd.arg("make")
        .arg("--tree")
        .arg(tree)
        .arg("--keys")
        .arg(keys)
        .arg("--out")
        .arg(out)
        .args(["--version", version])
        .args(["--updates-from", updates_from])
        .args(["--commit", &commit(ctx)])
        .args(["--toolchain", &toolchain(ctx)?])
        .args(["--built", &crate::image::image_time(ctx).to_string()]);
    process::run(ctx, &mut cmd)
}

/// Runs system-release, built for this machine, with `args`: for an
/// update repository's commands (ADR 0466).
pub fn system_release<S: AsRef<std::ffi::OsStr>>(ctx: &Ctx, args: &[S]) -> Result<()> {
    let mut cmd = Command::new(tool(ctx)?);
    cmd.args(args);
    process::run(ctx, &mut cmd)
}

/// Writes Rescue of the release in the repository `repo`, which this
/// machine's keys signed, into `esp`, a folder staged as an EFI system
/// partition: `boot/rescue` (ADR 0460).
pub fn rescue(ctx: &Ctx, repo: &Path, esp: &Path) -> Result<()> {
    let root = keys(ctx)?.join("root.json");
    step(&format!("Writing Rescue into {}", ctx.display(esp)));
    let mut cmd = Command::new(tool(ctx)?);
    cmd.arg("rescue").arg("--root").arg(&root).arg("--out").arg(esp).arg(repo);
    process::run(ctx, &mut cmd)
}

/// The commit the tree is at, or `unknown` outside a git checkout.
fn commit(ctx: &Ctx) -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&ctx.root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".into(), |text| text.trim().to_string())
}

/// The toolchain rust-toolchain.toml pins, such as `nightly-2026-10-15`.
fn toolchain(ctx: &Ctx) -> Result<String> {
    let path = ctx.root.join("rust-toolchain.toml");
    let text = std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
    let file: toml::Value = toml::from_str(&text)
        .map_err(|err| crate::Error::new(format!("{}: {err}", path.display())))?;
    file.get("toolchain")
        .and_then(|t| t.get("channel"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| crate::Error::new(format!("{} names no channel", path.display())))
}

#[cfg(test)]
mod tests {
    use super::canary_words_are_valid as valid;
    use super::*;

    /// A tree made in the order `names` gives, whatever order the folder
    /// then lists in.
    fn tree(dir: &Path, names: &[&str]) {
        let _ = std::fs::remove_dir_all(dir);
        for name in names {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            if let Some(target) = name.strip_suffix("-link") {
                std::os::unix::fs::symlink(Path::new(target).file_name().unwrap(), &path).unwrap();
            } else {
                std::fs::write(&path, format!("the file {name}\n")).unwrap();
                let file = std::fs::File::options().write(true).open(&path).unwrap();
                let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_793_592_777);
                file.set_modified(when).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
            }
        }
    }

    #[test]
    fn the_file_list_is_sorted_and_one_line_for_each_folder_file_and_link() {
        let dir = std::env::temp_dir().join(format!("ob-file-list-{}", std::process::id()));
        let (a, b) = (dir.join("a"), dir.join("b"));
        // A folder's mode is the umask's unless it is set.
        let folder = |tree: &Path| {
            let mode = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(tree.join("commands"), mode).unwrap();
        };
        tree(&a, &["commands/zstd", "commands/ls", "commands/ls-link", "a.txt"]);
        folder(&a);
        let file = |name: &str| {
            let text = format!("the file {name}\n");
            let sum = hex(&Sha256::digest(text.as_bytes()));
            format!("f\t0640\t{sum}\t{}\t1793592777\t{name}", text.len())
        };
        let wanted = [
            file("a.txt"),
            "d\t0755\t-\t-\t-\tcommands".to_string(),
            file("commands/ls"),
            "l\t0777\t-\t-\t-\tcommands/ls-link\tls".to_string(),
            file("commands/zstd"),
        ];
        let listed = file_list(&a).unwrap();
        assert_eq!(listed, wanted.join("\n") + "\n");
        // The same tree made in another order lists the same.
        tree(&b, &["a.txt", "commands/ls-link", "commands/ls", "commands/zstd"]);
        folder(&b);
        assert_eq!(file_list(&b).unwrap(), listed);
        // A change of one byte shows.
        std::fs::write(b.join("a.txt"), "the file a.txt!\n").unwrap();
        assert_ne!(file_list(&b).unwrap(), listed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_canary_words_are_the_scripts_and_a_colour_needs_its_reason() {
        assert!(valid(&["add", "ref-box", "ab12"]));
        assert!(valid(&["remove", "ref-box"]));
        assert!(valid(&["ref-box", "auto"]));
        assert!(valid(&["ref-box", "red", "disk", "swap"]));
        assert!(valid(&["ref-box", "green", "known fault"]));
        assert!(!valid(&[]));
        assert!(!valid(&["ref-box"]));
        assert!(!valid(&["ref-box", "red"]));
        assert!(!valid(&["ref-box", "green"]));
        assert!(!valid(&["add", "ref-box"]));
        assert!(!valid(&["remove"]));
        assert!(!valid(&["ref-box", "blue", "why"]));
    }
}
