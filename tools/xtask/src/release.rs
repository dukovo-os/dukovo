// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Signed system releases, `ob`'s side (ADR 0308): the keys that sign
//! them, the root the boot archive carries so that a system trusts them,
//! and the release of a staged slot, made and signed by system-release,
//! which ob builds for this machine as it builds catalog-repo. `ob
//! release` deploys an update repository to the update server and checks
//! what it serves (ADR 0467).
//!
//! Until releases are signed with the project's YubiKeys (the updates
//! design's section 11), every machine that builds Dukovo signs with keys
//! of its own, made the first time they are needed in
//! `.cache/release-keys`: a stick verifies against the root built into
//! it, and nothing online trusts that root. When a stick's release will not check, run
//! `system-release check` on the stick's `release` folder with
//! `--root .cache/release-keys/root.json`.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::deploy::{self, Server};
use crate::process::{self, step};
use crate::{Context, Ctx, Result, VERSION, bail};

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
        ["server", "setup"] => crate::catalog::server_setup(ctx, false),
        ["server", "setup", "--dukovo"] => crate::catalog::server_setup(ctx, true),
        ["deploy", dir] => deploy(ctx, Path::new(dir)),
        ["send", dir] => publish_to(ctx, &Server::from_environment(), Path::new(dir)),
        ["send", "--copy-only", dir] => copy_to(ctx, &Server::from_environment(), Path::new(dir)),
        ["check"] => check(ctx, None, URL),
        ["check", url] => check(ctx, None, url),
        ["check", "--root", root, url] => check(ctx, Some(Path::new(root)), url),
        ["rollout", rest @ ..] if !rest.is_empty() => rollout(ctx, rest),
        _ => bail!(
            "usage: ./ob release server setup [--dukovo] | deploy <repository> \
             | send [--copy-only] <repository> | check [--root <file>] [<url>] \
             | rollout (pause <version> <reason> | resume <version> | canary <name> red|green)"
        ),
    }
}

/// `ob release rollout ...`: on the update server, as its user (through
/// sudo, or through elevate on a Dukovo server), pauses or resumes a
/// release's waves, or marks a canary red or green, and signs the
/// rollouts again (ADR 0469); needs no release key.
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

/// The production first root, which every repository deployed must start
/// from.
fn production_root(ctx: &Ctx) -> Result<Vec<u8>> {
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

/// The root a system trusts releases by, for the boot archive: its path
/// there, [`TRUSTED_ROOT`], and a copy of this machine's, readable by
/// all.
pub fn trusted_root(ctx: &Ctx) -> Result<(String, PathBuf)> {
    let from = keys(ctx)?.join("root.json");
    let copy = ctx.out.join("release/root.json");
    process::copy(&from, &copy)?;
    std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o644))
        .context(|| format!("setting the mode of {}", copy.display()))?;
    Ok((TRUSTED_ROOT.to_string(), copy))
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
