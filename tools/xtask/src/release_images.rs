// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob release images VERSION`: the project's install stick and ISO of a
//! release that `ob release sign` signed and `ob release deploy` sent to
//! updates.dukovo.org (ADR 0779). They trust the release root,
//! `releases/metadata/1.root.json`, so that a system installed from one
//! takes its updates from the server, and they carry the release as the
//! server serves it, so that nothing on them is signed by this machine.
//!
//! It refuses unless the checkout has nothing changed and builds this
//! version. It builds the release, as `ob release build` does, and
//! system-release lays out what the server serves for it, read from the
//! first root, which must be the release built here, byte for byte: a
//! fourth build of the release three builds agreed on. Then it builds the
//! stick and the ISO around that, writes their `SHA256SUMS`, shows them,
//! asks for the version to be typed, and signs the checksums on a YubiKey
//! with its PIN and a touch, and sends the signature to Sigstore's Rekor
//! log. Everything is left in `out/release-images/VERSION`.
//!
//! When it stops at the server's release, the release is not deployed
//! yet, or this checkout is not its commit; when it stops at the YubiKey,
//! the root does not name the YubiKey's key, as for `ob release sign`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Profile;
use crate::image::{self, Kind, Overrides};
use crate::process::{self, step};
use crate::release::{self, BUILD_FILES, BUILD_TREE, REPO, URL};
use crate::release_sign::{record_entry, unchanged};
use crate::{Context, Ctx, Result, VERSION, bail};

/// The images a release is installed from, in the order they are made.
const KINDS: [Kind; 2] = [Kind::Stick, Kind::Iso];

/// What `images` is asked for: the version, and the YubiKey that signs.
#[derive(Debug, PartialEq, Eq)]
struct Request<'a> {
    version: &'a str,
    serial: Option<&'a str>,
}

fn request<'a>(args: &[&'a str]) -> Result<Request<'a>> {
    match args {
        [version] if !version.starts_with('-') => Ok(Request { version, serial: None }),
        [version, "--serial", serial] if !version.starts_with('-') => {
            Ok(Request { version, serial: Some(serial) })
        }
        _ => bail!("usage: ./ob release images <version> [--serial <number>]"),
    }
}

/// Where `images` leaves the images of each version.
fn images_dir(ctx: &Ctx) -> PathBuf {
    ctx.out.join("release-images")
}

/// The Rekor entry of the signature of `version`'s images' checksums,
/// and where the log's answer is kept.
pub(crate) fn entry_paths(ctx: &Ctx, version: &str) -> (PathBuf, PathBuf) {
    let dir = images_dir(ctx).join(version);
    (dir.join("SHA256SUMS.rekor-entry.json"), dir.join("SHA256SUMS.rekor.json"))
}

/// `ob release images VERSION [--serial N]`.
pub fn images(ctx: &Ctx, args: &[&str]) -> Result<()> {
    let request = request(args)?;
    let version = request.version;
    if version != VERSION {
        bail!(
            "this checkout builds Dukovo {VERSION}, not {version}: a release's images are built \
             from the commit it was signed from"
        );
    }
    release::production_root(ctx)?;
    let out = images_dir(ctx).join(version);
    if out.exists() {
        bail!("{} is there already: remove it to make the images again", ctx.display(&out));
    }
    unchanged(ctx)?;
    let work = images_dir(ctx).join(format!("{version}.work"));
    if work.exists() {
        std::fs::remove_dir_all(&work).context(|| format!("removing {}", work.display()))?;
    }
    let made = make(ctx, &request, &work, &out);
    let _ = std::fs::remove_dir_all(&work);
    if let Err(err) = made {
        // Nothing half made is left to be taken for the images.
        let _ = std::fs::remove_dir_all(&out);
        return Err(err);
    }
    let (entry, answer) = entry_paths(ctx, version);
    let what = format!("the checksums of Dukovo {version}'s install images");
    record_entry(ctx, version, &what, &entry, &answer).map_err(|err| {
        err.hint(format!(
            "the images are made and their checksums signed: `./ob release record {version}` \
             records the signature"
        ))
    })?;
    println!("Dukovo {version}'s install images are in {}.", ctx.display(&out));
    Ok(())
}

/// The heart of [`images`], working in `work` and writing into `out`.
fn make(ctx: &Ctx, request: &Request, work: &Path, out: &Path) -> Result<()> {
    let built = work.join("build");
    release::build_release(ctx, &built)?;
    // The build pinned the release root for every build of this run: the
    // live system trusts it, as the release does.
    let root = release::production_root(ctx)?;
    let (_, trusted) = release::trusted_root(ctx)?;
    if std::fs::read(&trusted).context(|| format!("reading {}", trusted.display()))? != root {
        bail!("the images would not trust {REPO}/metadata/1.root.json");
    }
    let medium = work.join("release");
    step(&format!("Reading Dukovo {} as {URL} serves it", request.version));
    let mut cmd = Command::new(release::tool(ctx)?);
    cmd.arg("medium")
        .arg("--root")
        .arg(ctx.root.join(REPO).join("metadata/1.root.json"))
        .arg("--tree")
        .arg(ctx.out.join(BUILD_TREE))
        .arg("--out")
        .arg(&medium)
        .arg(URL)
        .arg(built.join(BUILD_FILES[1]));
    process::run(ctx, &mut cmd)?;
    process::create_dir(out)?;
    let overrides = Overrides { release: Some(medium.clone()), ..Overrides::default() };
    let mut made = Vec::new();
    for kind in KINDS {
        let image = image::build_release(ctx, Profile::Release, kind, &overrides)?;
        let to = out.join(image::file_name(kind));
        std::fs::rename(&image, &to)
            .context(|| format!("moving {} to {}", image.display(), to.display()))?;
        made.push(to);
    }
    let mut sums = Command::new(release::tool(ctx)?);
    sums.arg("sums")
        .arg("--root")
        .arg(medium.join("metadata/root.json"))
        .arg("--yubikey-helper")
        .arg(ctx.root.join(crate::catalog::YUBIKEY_HELPER))
        .args(request.serial.map(|serial| ["--serial", serial]).into_iter().flatten())
        .args(["--version", request.version])
        .arg("--out")
        .arg(out)
        .arg("--record")
        .arg(entry_paths(ctx, request.version).0)
        .args(&made);
    // The terminal, for the version to be typed and the YubiKey's PIN.
    process::run(ctx, &mut sums)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_images_are_asked_for_by_version_and_signed_on_a_yubikey_named_or_plugged_in() {
        let plugged = Request { version: "1.0.0-alpha.1", serial: None };
        assert_eq!(request(&["1.0.0-alpha.1"]).unwrap(), plugged);
        let named = Request { version: "1.0.0-alpha.1", serial: Some("38775519") };
        assert_eq!(request(&["1.0.0-alpha.1", "--serial", "38775519"]).unwrap(), named);
        for wrong in [&[][..], &["--serial", "1"], &["1.0.0", "--serial"], &["1.0.0", "x"]] {
            assert!(request(wrong).is_err(), "{wrong:?}");
        }
    }

    /// The release keys' PEM files, which a download's checksums are
    /// checked with, are the keys the key ceremony kept.
    #[test]
    fn each_release_keys_pem_is_the_key_its_file_holds() {
        let keys = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(REPO).join("keys");
        let mut checked = 0;
        for entry in std::fs::read_dir(&keys).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            if !name.starts_with("yubikey-") || path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let held: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let pem = std::fs::read_to_string(path.with_extension("pem")).unwrap();
            assert_eq!(held["key"]["keyval"]["public"].as_str(), Some(pem.as_str()), "{name}");
            checked += 1;
        }
        // Two, or one between a YubiKey's loss and its replacement.
        assert!((1..=2).contains(&checked), "{checked} YubiKeys' keys");
    }
}
