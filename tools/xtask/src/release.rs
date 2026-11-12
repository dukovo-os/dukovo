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

/// The server's folder a deploy copies into (tools/update-server/).
const INCOMING: &str = "/var/lib/dukovo-system-incoming";

/// The repository the server serves, whose store a deploy skips the
/// files of.
const SERVED: &str = "/srv/dukovo-system/system/";

/// The online key's public half, as the server's setup leaves it.
pub const SERVER_ONLINE_KEY: &str = "/usr/local/share/dukovo-system/online.json";

/// How long a deploy waits for the server to publish: it hashes every
/// new file as it takes it.
const PUBLISH_WAIT_SECONDS: u32 = 600;

/// Builds system-release for this machine and gives its path.
fn tool(ctx: &Ctx) -> Result<PathBuf> {
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
/// to it, or checks what it serves.
pub fn command(ctx: &Ctx, args: &[String]) -> Result<()> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["server", "setup"] => crate::catalog::server_setup(ctx),
        ["deploy", dir] => deploy(ctx, Path::new(dir)),
        ["check"] => check(ctx, URL),
        ["check", url] => check(ctx, url),
        _ => bail!("usage: ./ob release server setup | deploy <repository> | check [<url>]"),
    }
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
    let dest = crate::catalog::ssh_destination();
    step(&format!("Copying the repository to {dest}"));
    // Made first, so that the server's user may read them, whatever the
    // login's umask.
    let folders =
        format!("mkdir -p -m 2770 {INCOMING}/metadata {INCOMING}/targets {INCOMING}/files");
    process::run(ctx, &mut crate::catalog::ssh(&dest, &folders))?;
    let mut cmd = Command::new("rsync");
    cmd.args(["-rtp", "--delete", "--chmod=D2770,F0660"])
        .arg(dir.join("metadata"))
        .arg(dir.join("targets"))
        .arg(format!("{dest}:{INCOMING}/"));
    process::run(ctx, &mut cmd)?;
    // Files are named by their SHA-256, so one the store holds is not
    // copied again, and none is ever taken away from it.
    let mut cmd = Command::new("rsync");
    cmd.args(["-rtp", "--size-only", "--chmod=D2770,F0660"])
        .arg(format!("--compare-dest={SERVED}"))
        .arg(dir.join("files"))
        .arg(format!("{dest}:{INCOMING}/"));
    process::run(ctx, &mut cmd)?;

    step("Publishing it on the server");
    let script = format!(
        "rm -f {INCOMING}/result && touch {INCOMING}/ready && \
         i=0; while [ ! -e {INCOMING}/result ] && [ $i -lt {PUBLISH_WAIT_SECONDS} ]; do \
         sleep 1; i=$((i + 1)); done; \
         if [ -e {INCOMING}/result ]; then cat {INCOMING}/result; else echo waiting; fi"
    );
    let output = process::output(ctx, &mut crate::catalog::ssh(&dest, &script))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (status, rest) = text.split_once('\n').unwrap_or((text.trim_end(), ""));
    if !rest.trim().is_empty() {
        println!("{}", rest.trim_end());
    }
    match status {
        "published" => {}
        "waiting" => bail!(
            "the server did not publish in {PUBLISH_WAIT_SECONDS} seconds: \
             `systemctl status dukovo-system-publish` on the server says why"
        ),
        _ => bail!(
            "the server could not publish the releases, as the output above says; it serves what \
             it did before"
        ),
    }
    check(ctx, URL)
}

/// `ob release check`: reads the repository `url` serves as a computer
/// online would, from the production first root: every release list and
/// channel.
fn check(ctx: &Ctx, url: &str) -> Result<()> {
    production_root(ctx)?;
    let root = ctx.root.join(REPO).join("metadata/1.root.json");
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
