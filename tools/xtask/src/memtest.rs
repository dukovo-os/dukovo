// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! memtest86+, built from its pinned source for the start menu's memory
//! test (ADR 0488): one program, `mt86plus`, that UEFI starts as an EFI
//! program and a BIOS through the Linux boot protocol, which GRUB starts
//! from a release's startup folder, from Rescue's copy of it, and from
//! the install stick.
//!
//! The release's tarball is fetched once, checked against its SHA-256,
//! and built with the build machine's GCC into `.cache/memtest/<version>`,
//! which holds the program and its license; [`RECIPE`] names what the
//! build depends on, so a change to it builds memtest86+ again. It is
//! built with `GIT=none`, so that the version it shows does not depend on
//! whether the build machine has git; nothing else in it names the time
//! or the machine it was built on.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result};

/// The release built.
pub const VERSION: &str = "8.10";

/// Its tarball's SHA-256: Debian's and Ubuntu's `orig` tarball, which is
/// the project's own tarball of release 8.10, commit 494689a.
pub const SHA256: &str = "5a823ed5da95bd86f5147c8faa8096a70fc47b212bb4a8e018929042cc368a1e";

/// Where the tarball is published: Ubuntu's archive.
pub const URL: &str =
    "https://archive.ubuntu.com/ubuntu/pool/main/m/memtest86+/memtest86+_8.10.orig.tar.gz";

/// Where the tarball is fetched from: Ubuntu's archive, then Debian's
/// copy of the same file, which [`SHA256`] checks is the same.
const URLS: &[&str] =
    &[URL, "https://deb.debian.org/debian/pool/main/m/memtest86+/memtest86+_8.10.orig.tar.gz"];

/// The directory the tarball unpacks into.
const SOURCE: &str = "memtest86plus-memtest86plus-494689a";

/// What a build depends on, kept beside it: when this changes, memtest86+
/// is built again.
const RECIPE: &str = "memtest86+ 8.10, 5a823ed5, make -C build/x86_64 GIT=none mt86plus, recipe 1";

/// memtest86+, built.
pub struct Memtest {
    /// The program, `mt86plus`.
    pub program: PathBuf,
    /// Its license, the source's `LICENSE`, the GNU GPL version 2.
    pub license: PathBuf,
}

/// The directory memtest86+ is built into: `.cache/memtest/<version>`.
fn cache(ctx: &Ctx) -> PathBuf {
    ctx.root.join(".cache/memtest").join(VERSION)
}

/// memtest86+, fetched and built the first time, and when [`RECIPE`]
/// changes.
///
/// # Errors
///
/// If it cannot be fetched or built: the tools its build needs (GCC,
/// make, GNU ld and objcopy) are missing, say.
pub fn build(ctx: &Ctx) -> Result<Memtest> {
    let dir = cache(ctx);
    let built = Memtest { program: dir.join("mt86plus"), license: dir.join("LICENSE") };
    let stamp = dir.join("recipe");
    if std::fs::read_to_string(&stamp).is_ok_and(|text| text == RECIPE)
        && built.program.exists()
        && built.license.exists()
    {
        return Ok(built);
    }
    for (tool, purpose) in [
        ("make", "build memtest86+"),
        ("gcc", "compile memtest86+"),
        ("ld", "link memtest86+"),
        ("objcopy", "make memtest86+'s program"),
        ("tar", "unpack memtest86+"),
    ] {
        if process::find_program(tool).is_none() {
            return Err(Error::new(format!("{tool} is needed to {purpose} but is not installed"))
                .hint("run `./ob doctor` to see what is missing"));
        }
    }
    let tarball = ctx.root.join(".cache/memtest").join(format!("memtest86+_{VERSION}.orig.tar.gz"));
    process::create_dir(tarball.parent().expect("the cache has a parent"))?;
    if !tarball.exists() || process::sha256(ctx, &tarball)? != SHA256 {
        fetch(ctx, &tarball)?;
    }
    let work = ctx.target.join("memtest");
    if work.exists() {
        std::fs::remove_dir_all(&work).context(|| format!("removing {}", work.display()))?;
    }
    process::create_dir(&work)?;
    step(&format!("Unpacking memtest86+ {VERSION}"));
    process::run(ctx, Command::new("tar").arg("-xzf").arg(&tarball).arg("-C").arg(&work))?;
    let source = work.join(SOURCE);
    let jobs = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    step(&format!("Building memtest86+ {VERSION}, with {jobs} jobs"));
    let mut make = Command::new("make");
    make.arg(format!("-j{jobs}"))
        .args(["-C", "build/x86_64", "GIT=none", "mt86plus"])
        .current_dir(&source);
    logged(ctx, &mut make, &work.join("make.log"))?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir).context(|| format!("removing {}", dir.display()))?;
    }
    process::create_dir(&dir)?;
    process::copy(&source.join("build/x86_64/mt86plus"), &built.program)?;
    process::copy(&source.join("LICENSE"), &built.license)?;
    std::fs::write(&stamp, RECIPE).context(|| format!("writing {}", stamp.display()))?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(built)
}

/// Runs `cmd` with its output in `log`, whose end a failure shows.
fn logged(ctx: &Ctx, cmd: &mut Command, log: &Path) -> Result<()> {
    let file = std::fs::File::create(log).context(|| format!("writing {}", log.display()))?;
    let err = file.try_clone().context(|| format!("writing {}", log.display()))?;
    cmd.stdin(Stdio::null()).stdout(file).stderr(err);
    let status = process::status(ctx, cmd)?;
    if status.success() {
        return Ok(());
    }
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let tail: Vec<&str> = text.lines().rev().take(20).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    Err(Error::new(format!(
        "building memtest86+ failed ({status}); the end of {}:\n{}",
        log.display(),
        tail.join("\n")
    )))
}

/// Downloads the pinned tarball to `path`, from the first of [`URLS`]
/// that serves it whole.
fn fetch(ctx: &Ctx, path: &Path) -> Result<()> {
    let partial = path.with_extension("partial");
    let mut failed = Vec::new();
    for url in URLS {
        step(&format!("Fetching memtest86+ {VERSION} from {url}"));
        let mut cmd = Command::new("curl");
        cmd.args(["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "3", "-o"])
            .arg(&partial)
            .arg(url);
        if let Err(err) = process::run(ctx, &mut cmd) {
            failed.push(format!("{url}: {err}"));
            continue;
        }
        let sum = process::sha256(ctx, &partial)?;
        if sum == SHA256 {
            return std::fs::rename(&partial, path)
                .context(|| format!("moving it to {}", path.display()));
        }
        failed.push(format!("{url} has SHA-256 {sum}, not the pinned {SHA256}"));
    }
    let _ = std::fs::remove_file(&partial);
    Err(Error::new(format!("no copy of memtest86+ {VERSION} would do: {}", failed.join("; "))))
}
