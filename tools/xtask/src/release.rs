// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Signed system releases, `ob`'s side (ADR 0308): the keys that sign
//! them, the root the boot archive carries so that a system trusts them,
//! and the release of a staged slot, made and signed by system-release,
//! which ob builds for this machine as it builds catalog-repo.
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
use crate::{Context, Ctx, Result, VERSION};

/// Where this machine's release keys are kept.
pub const KEYS_DIR: &str = ".cache/release-keys";

/// Where the boot archive carries the root a system trusts releases by,
/// which system-release(8) checks against by default.
pub const TRUSTED_ROOT: &str = "system/resources/updates/root.json";

/// Where a release's repository is on the install stick's DUKOVO_LIVE
/// partition: its `release` subvolume, read-only.
pub const SUBVOLUME: &str = "release";

/// Builds system-release for this machine and gives its path.
fn tool(ctx: &Ctx) -> Result<PathBuf> {
    step("Building system-release for this machine");
    let mut cmd = crate::userspace::host_cargo_of(ctx, "build", &["system-release"]);
    cmd.args(["--bin", "system-release"]);
    process::run(ctx, &mut cmd)?;
    Ok(ctx.target.join("programs-host/debug/system-release"))
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
    let keys = keys(ctx)?;
    step(&format!("Signing release {version} into {}", ctx.display(out)));
    let mut cmd = Command::new(tool(ctx)?);
    cmd.arg("make")
        .arg("--tree")
        .arg(tree)
        .arg("--keys")
        .arg(&keys)
        .arg("--out")
        .arg(out)
        .args(["--version", version])
        .args(["--updates-from", updates_from])
        .args(["--commit", &commit(ctx)])
        .args(["--toolchain", &toolchain(ctx)?])
        .args(["--built", &crate::image::image_time(ctx).to_string()]);
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
