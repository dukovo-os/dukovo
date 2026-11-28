// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Linux desktop `ob bench --desktop` measures Dukovo's beside
//! (ADR 0724): labwc, a stacking compositor on wlroots that draws its
//! windows' frames itself, with wlroots' pixman renderer, so that both
//! desktops draw on the processor.
//!
//! Its programs and libraries are Ubuntu 24.04's, from the release
//! pocket, whose packages do not change, each pinned by the SHA-256 the
//! archive's index gives it (`linux_desktop.pins`). `ob bench` fetches
//! them once into `.cache/linux-desktop`, unpacks them without
//! installing them, and copies the files the list names into one tree,
//! which goes into the Linux virtual machine's initramfs. Nothing of
//! them is kept in the source tree or goes into an image of Dukovo.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::process::{self, step};
use crate::{Ctx, Error, Result, baseline};

/// The pinned packages, and what is taken from each.
const PINS: &str = include_str!("linux_desktop.pins");

/// Where labwc's loader is looked for, and where its package has it.
const LOADER: (&str, &str) =
    ("lib64/ld-linux-x86-64.so.2", "usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2");

/// The X server labwc looks for when it starts, and would start for a
/// program that speaks X11. None does: the file is there to be found,
/// and is no X server.
const XWAYLAND: &str = "usr/bin/Xwayland";

/// The most links [`resolved`] follows from one name.
const LINKS_MOST: usize = 8;

/// One pinned package.
#[derive(Debug, PartialEq, Eq)]
struct Package<'a> {
    sha256: &'a str,
    /// Its file's place in the archive.
    pool: &'a str,
    /// The files and folders taken from it, by their paths in it.
    takes: Vec<&'a str>,
}

impl Package<'_> {
    /// The package's file name.
    fn file(&self) -> &str {
        self.pool.rsplit('/').next().unwrap_or(self.pool)
    }
}

/// The packages `text` lists, a line each; a `#` starts a comment.
fn packages(text: &str) -> Result<Vec<Package<'_>>> {
    let mut packages = Vec::new();
    for line in text.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let mut words = line.split_whitespace();
        let (Some(sha256), Some(pool)) = (words.next(), words.next()) else {
            return Err(Error::new(format!("linux_desktop.pins: `{line}` names no package")));
        };
        let takes: Vec<&str> = words.collect();
        let pinned = sha256.len() == 64 && sha256.bytes().all(|byte| byte.is_ascii_hexdigit());
        let inside = |path: &&str| {
            Path::new(path).components().all(|part| matches!(part, Component::Normal(_)))
        };
        let a_deb = Path::new(pool).extension().is_some_and(|kind| kind == "deb");
        if !pinned || !a_deb || takes.is_empty() || !takes.iter().all(inside) {
            return Err(Error::new(format!(
                "linux_desktop.pins: `{line}` is not a package's line"
            )));
        }
        packages.push(Package { sha256, pool, takes });
    }
    Ok(packages)
}

/// The tree of the Linux desktop's files, as they lie from `/`: fetched,
/// checked and put together once, and again when the list changes.
pub fn tree(ctx: &Ctx) -> Result<PathBuf> {
    let cache = ctx.root.join(".cache/linux-desktop");
    let (tree, done) = (cache.join("tree"), cache.join("pins"));
    if tree.is_dir() && std::fs::read_to_string(&done).is_ok_and(|was| was == PINS) {
        return Ok(tree);
    }
    let Some(dpkg_deb) = process::find_program("dpkg-deb") else {
        return Err(Error::new(
            "the Linux desktop is Ubuntu's packages, and unpacking them takes dpkg-deb",
        )
        .hint("install dpkg, or measure Dukovo's side alone with --no-linux"));
    };
    let packages = packages(PINS)?;
    let (debs, unpacked, partial) =
        (cache.join("debs"), cache.join("unpacked"), cache.join("tree.partial"));
    for stale in [&tree, &unpacked, &partial] {
        let _ = std::fs::remove_dir_all(stale);
    }
    let _ = std::fs::remove_file(&done);
    process::create_dir(&debs)?;
    process::create_dir(&partial)?;
    step(&format!("Fetching the Linux desktop: {} of Ubuntu's packages, pinned", packages.len()));
    for package in &packages {
        let deb = debs.join(package.file());
        if !(deb.exists() && process::sha256(ctx, &deb)? == package.sha256) {
            download(ctx, package, &deb)?;
        }
        // Each package by itself: a link is followed inside the package
        // that has it.
        let root = unpacked.join(package.file());
        process::create_dir(&root)?;
        process::output(ctx, Command::new(&dpkg_deb).arg("-x").arg(&deb).arg(&root))?;
        for path in &package.takes {
            take(&root, Path::new(path), &partial)?;
        }
    }
    let (loader_at, loader) = LOADER;
    process::copy(&partial.join(loader), &partial.join(loader_at))?;
    let xwayland = partial.join(XWAYLAND);
    std::fs::write(&xwayland, "#!/bin/false\n")
        .and_then(|()| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&xwayland, std::fs::Permissions::from_mode(0o755))
        })
        .map_err(|err| Error::new(format!("writing {}: {err}", xwayland.display())))?;
    let _ = std::fs::remove_dir_all(&unpacked);
    std::fs::rename(&partial, &tree)
        .map_err(|err| Error::new(format!("moving the tree to {}: {err}", tree.display())))?;
    std::fs::write(&done, PINS)
        .map_err(|err| Error::new(format!("writing {}: {err}", done.display())))?;
    Ok(tree)
}

/// Downloads `package` to `deb`, from the first of Ubuntu's archives
/// that serves it whole.
fn download(ctx: &Ctx, package: &Package<'_>, deb: &Path) -> Result<()> {
    let partial = deb.with_extension("partial");
    let mut failed = Vec::new();
    for archive in baseline::ARCHIVES {
        let url = format!("{archive}/{}", package.pool);
        let mut cmd = Command::new("curl");
        cmd.args(["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "3", "-o"])
            .arg(&partial)
            .arg(&url);
        if let Err(err) = process::output(ctx, &mut cmd) {
            failed.push(format!("{url}: {err}"));
            continue;
        }
        let sum = process::sha256(ctx, &partial)?;
        if sum == package.sha256 {
            return std::fs::rename(&partial, deb)
                .map_err(|err| Error::new(format!("moving it to {}: {err}", deb.display())));
        }
        failed.push(format!("{url} has SHA-256 {sum}, not the pinned {}", package.sha256));
    }
    let _ = std::fs::remove_file(&partial);
    Err(Error::new(format!("no copy of {} would do: {}", package.file(), failed.join("; ")))
        .hint("measure Dukovo's side alone with --no-linux"))
}

/// The file or folder that `path`, beneath `root`, is or leads to: a
/// link is followed, and one that starts at `/` starts at `root`, as it
/// would on the system the package is for.
fn resolved(root: &Path, path: &Path) -> Result<PathBuf> {
    let mut at = root.join(path);
    for _ in 0..LINKS_MOST {
        let Ok(target) = std::fs::read_link(&at) else { return Ok(at) };
        at = match target.strip_prefix("/") {
            Ok(from_root) => root.join(from_root),
            Err(_) => at.parent().unwrap_or(root).join(target),
        };
    }
    Err(Error::new(format!(
        "{} is a link to a link, too many times over",
        root.join(path).display()
    )))
}

/// Copies `path` of the package unpacked at `root` to the same path
/// beneath `tree`: a file, or a folder with all it holds.
fn take(root: &Path, path: &Path, tree: &Path) -> Result<()> {
    let from = resolved(root, path)?;
    let meta = std::fs::metadata(&from).map_err(|err| {
        Error::new(format!("{} of the package at {}: {err}", path.display(), root.display()))
    })?;
    if !meta.is_dir() {
        return process::copy(&from, &tree.join(path));
    }
    let listed = std::fs::read_dir(&from)
        .map_err(|err| Error::new(format!("reading {}: {err}", from.display())))?;
    for entry in listed {
        let entry =
            entry.map_err(|err| Error::new(format!("reading {}: {err}", from.display())))?;
        take(root, &path.join(entry.file_name()), tree)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_list_is_read_and_has_the_compositor_and_its_loader() {
        let packages = packages(PINS).unwrap();
        let takes = |path: &str| packages.iter().any(|package| package.takes.contains(&path));
        assert!(takes("usr/bin/labwc") && takes(LOADER.1) && takes("usr/share/X11/xkb"));
        // No package is listed twice.
        let mut files: Vec<&str> = packages.iter().map(Package::file).collect();
        files.sort_unstable();
        let listed = files.len();
        files.dedup();
        assert_eq!(files.len(), listed);
    }

    #[test]
    fn a_line_that_is_no_package_s_is_refused() {
        let sum = "0".repeat(64);
        let good =
            format!("# a comment\n\n{sum} pool/main/a/a/a_1_amd64.deb usr/lib/a.so.1 etc/a\n");
        let read = packages(&good).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!((read[0].file(), read[0].takes.len()), ("a_1_amd64.deb", 2));
        for bad in [
            format!("{sum}\n"),
            format!("{sum} pool/main/a/a/a_1_amd64.deb\n"),
            format!("{} pool/main/a/a/a_1_amd64.deb usr/lib/a.so.1\n", &sum[1..]),
            format!("{sum} pool/main/a/a/a_1_amd64.tar usr/lib/a.so.1\n"),
            format!("{sum} pool/main/a/a/a_1_amd64.deb /usr/lib/a.so.1\n"),
            format!("{sum} pool/main/a/a/a_1_amd64.deb usr/../../etc/passwd\n"),
        ] {
            assert!(packages(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_taken_link_is_the_file_it_leads_to_inside_the_package() {
        let dir = std::env::temp_dir().join(format!("ob-linux-desktop-{}", std::process::id()));
        let (root, tree) = (dir.join("root"), dir.join("tree"));
        std::fs::create_dir_all(root.join("usr/lib")).unwrap();
        std::fs::create_dir_all(root.join("etc/conf.d")).unwrap();
        std::fs::write(root.join("usr/lib/a.so.1.2"), "a").unwrap();
        std::os::unix::fs::symlink("a.so.1.2", root.join("usr/lib/a.so.1")).unwrap();
        // A link from the system's root is followed from the package's.
        std::os::unix::fs::symlink("/usr/lib/a.so.1", root.join("etc/conf.d/a")).unwrap();
        take(&root, Path::new("usr/lib/a.so.1"), &tree).unwrap();
        take(&root, Path::new("etc"), &tree).unwrap();
        assert_eq!(std::fs::read_to_string(tree.join("usr/lib/a.so.1")).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(tree.join("etc/conf.d/a")).unwrap(), "a");
        assert!(!tree.join("usr/lib/a.so.1.2").exists());
        // One that leads nowhere is an error, not a file left out.
        std::os::unix::fs::symlink("gone", root.join("usr/lib/b.so.1")).unwrap();
        assert!(take(&root, Path::new("usr/lib/b.so.1"), &tree).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
