// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `sysupdate verify`: reads the running slot back against its release's
//! signed list, as a stage reads a slot it builds (ADR 0451): every
//! directory, file and link, each file's size and SHA-256, and each
//! mode. Btrfs checks every block it reads against its checksum, but
//! not that the files are the release's; this does.
//!
//! The signed list comes from where the owner names, a repository folder
//! or an update file; or else from Rescue's copy on the EFI system
//! partition, when it holds the same release, as it does after the
//! install and once each stable release proves itself (ADR 0460); or
//! else from the update server. Each is checked against the root this
//! system trusts first. updated(8) runs it once a week.

use std::path::{Path, PathBuf};
use std::process::Command;

use system_release::list::Release;

use super::online::{self, Settings};
use super::place::{self, Root};
use super::stage::{self, TRUSTED_ROOT};
use crate::util::{self, Fail, fail};

/// Where the EFI system partition is mounted, read-only, to read
/// Rescue's signed list.
const ESP: &str = "/tmp/sysupdate-verify/esp";

/// Where the time of the last verify is kept, for updated's weekly one.
pub const LAST: &str = "/system/state/sysupdate/verified";

/// How long updated waits between verifies.
pub const EVERY_SECONDS: i64 = 7 * 24 * 60 * 60;

/// `sysupdate verify [REPOSITORY|FILE]`.
///
/// # Errors
///
/// If no signed list of the running release can be had, or the slot is
/// not the release it lists: then the first ten differences.
pub fn main(repo: Option<&Path>) -> Result<i32, Fail> {
    let root = place::root()?;
    let version = stage::running_version(&root)?;
    let (release, from) = signed_list(&root, &version, repo)?;
    let named = place::named(&root, root.running);
    stage::say(&format!("reading {named} back against the signed list from {from}"));
    stage::check(&root.slot(root.running), &release)?;
    let now = util::now_ns() / 1_000_000_000;
    if let Some(dir) = Path::new(LAST).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(LAST, format!("{now}\n"));
    println!(
        "sysupdate: {named} is the release its signed list names: {} entries read back",
        release.files.len()
    );
    Ok(0)
}

/// Whether a week has passed since the last verify, by [`LAST`].
#[must_use]
pub fn due(now: i64) -> bool {
    let last = std::fs::read_to_string(LAST)
        .ok()
        .and_then(|text| text.trim().parse::<i64>().ok())
        .unwrap_or(0);
    now - last >= EVERY_SECONDS
}

/// The signed list of Dukovo `version`, and where it came from.
fn signed_list(root: &Root, version: &str, repo: Option<&Path>) -> Result<(Release, String), Fail> {
    let trusted = std::fs::read(TRUSTED_ROOT).map_err(|err| util::failed(&TRUSTED_ROOT, err))?;
    if let Some(repo) = repo {
        let published = if repo.is_file() {
            system_release::updates::open_file(repo, &trusted, Some(version)).map(|(p, _)| p)
        } else {
            system_release::repo::open(repo, &trusted, Some(version))
        }
        .map_err(fail)?;
        return Ok((published.release, repo.display().to_string()));
    }
    match rescue_list(root, &trusted) {
        Ok(release) if release.version == version => {
            return Ok((release, "Rescue's copy".into()));
        }
        Ok(release) => stage::say(&format!(
            "Rescue holds Dukovo {}, not this system's {version}: asking the update server",
            release.version
        )),
        Err(why) => stage::say(&format!("{why}: asking the update server")),
    }
    let mut client = online::Client::connect(Settings::load()?)?;
    let server = client.settings.server.clone();
    let release = client
        .release(version)?
        .ok_or_else(|| fail(format!("{server} does not offer Dukovo {version}")))?;
    Ok((release, server))
}

/// The release Rescue holds, from the first EFI system partition of the
/// root's disks with Rescue, mounted read-only for the while.
fn rescue_list(root: &Root, trusted: &[u8]) -> Result<Release, String> {
    std::fs::create_dir_all(ESP).map_err(|err| format!("{ESP}: {err}"))?;
    for name in place::partitions(root).map_err(|why| why.message)? {
        let device = format!("/devices/{name}");
        if !run("mount", &["-t", "fat", "-o", "ro", &device, ESP]) {
            continue;
        }
        let dir: PathBuf = Path::new(ESP).join(system_release::rescue::DIR);
        let opened =
            dir.join("release").is_dir().then(|| system_release::rescue::open(&dir, trusted));
        run("umount", &[ESP]);
        if let Some(opened) = opened {
            return opened.map(|published| published.release);
        }
    }
    Err("no EFI system partition of the root's disks holds Rescue".into())
}

/// Runs `program` from /system/commands with `args`, quietly; whether it
/// succeeded.
fn run(program: &str, args: &[&str]) -> bool {
    Command::new(format!("/system/commands/{program}"))
        .args(args)
        .output()
        .is_ok_and(|out| out.status.success())
}
