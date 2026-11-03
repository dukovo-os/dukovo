// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Keeping ports up with their upstreams (ADR 0370).
//!
//! - `ob ports outdated` asks each port's git repository for its tags and
//!   lists the ports whose upstream has released a newer version than the
//!   tag the recipe pins. CI runs it each week and opens an issue for each
//!   port it lists; a person, who signs the commit off, moves the pin.
//! - `ob ports audit` checks the Cargo.lock of each bundle's port in Rust,
//!   with the port's patches, against the RustSec advisory database with
//!   cargo-audit, which CI runs each day, so that a flaw in a crate a
//!   bundle links is known the day it is published. A vulnerability fails
//!   it; a crate that is only unmaintained or unsound is shown, not
//!   failed on, since a port often cannot leave it.

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::process::Command;

use crate::ports::{self, Recipe};
use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result};

/// What `ob ports outdated` found of one port.
#[derive(Debug, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub pinned: String,
    /// The newest release upstream, when it is newer than the pin.
    pub newer: Option<String>,
}

/// `ob ports outdated`: the ports pinned to a tag of git whose upstream
/// has a newer release. With `tsv`, one `name\tpinned\tnewer` line each,
/// for CI; else a table, with every port it checked. Ports of a release
/// archive and the toolchain's are not checked.
pub fn outdated(ctx: &Ctx, tsv: bool) -> Result<()> {
    let mut found = Vec::new();
    let mut failed = Vec::new();
    for (recipe, _) in ports::recipes(ctx)? {
        let Some(tag) = recipe.source.tag.clone() else { continue };
        if recipe.source.git.is_empty() {
            continue;
        }
        match tags(ctx, &recipe.source.git) {
            Ok(tags) => found.push(Found {
                newer: newest(&tag, &tags).filter(|newest| newest != &tag),
                name: recipe.name.clone(),
                pinned: tag,
            }),
            Err(err) => failed.push(format!("{}: {err}", recipe.name)),
        }
    }
    let mut out = String::new();
    if tsv {
        for port in found.iter().filter(|p| p.newer.is_some()) {
            let _ = writeln!(
                out,
                "{}\t{}\t{}",
                port.name,
                port.pinned,
                port.newer.as_deref().unwrap_or("")
            );
        }
    } else {
        let width = found.iter().map(|p| p.name.len()).max().unwrap_or(0);
        for port in &found {
            let state =
                port.newer.as_ref().map_or("up to date".to_string(), |n| format!("{n} is out"));
            let _ = writeln!(out, "{:width$}  {:12}  {state}", port.name, port.pinned);
        }
    }
    print!("{out}");
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Error::new(format!("could not ask upstream of:\n  {}", failed.join("\n  "))))
    }
}

/// The tags of the repository `git`, by name.
fn tags(ctx: &Ctx, git: &str) -> Result<Vec<String>> {
    let mut cmd = Command::new("git");
    cmd.args(["ls-remote", "--tags", "--refs", git]);
    let output = process::output(ctx, &mut cmd)?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split('\t').nth(1)?.strip_prefix("refs/tags/").map(str::to_string))
        .collect())
}

/// A release tag's numbers, and what comes before them (such as `v` or
/// `curl-`), which a newer tag of the same series shares; none for a tag
/// that is not a plain release, such as `1.0-rc1` or `nightly`. A letter
/// after the numbers, as in tzdata's 2026f, counts as one more number.
fn release(tag: &str) -> Option<(&str, Vec<u64>)> {
    let start = tag.find(|c: char| c.is_ascii_digit())?;
    let (prefix, rest) = tag.split_at(start);
    let (rest, letter) = match rest.strip_suffix(|c: char| c.is_ascii_lowercase()) {
        Some(numbers) => (numbers, rest.bytes().last().map(|b| u64::from(b - b'a' + 1))),
        None => (rest, None),
    };
    let numbers: Option<Vec<u64>> = rest.split(['.', '_']).map(|part| part.parse().ok()).collect();
    let mut numbers = numbers.filter(|n| !n.is_empty())?;
    numbers.extend(letter);
    Some((prefix, numbers))
}

/// Whether `numbers` are a release in the series of `base`: any numbers
/// beyond base's are small, as in 1.2.5.1; not a snapshot's date
/// (ncurses' 6_6_20261031) or a development version (glibc's 2.42.9000).
fn same_series(numbers: &[u64], base: &[u64]) -> bool {
    numbers.iter().skip(base.len()).all(|&n| n < 1000)
}

/// The newest of `tags` in `pinned`'s series, if any tag is newer than or
/// equal to it.
fn newest(pinned: &str, tags: &[String]) -> Option<String> {
    let (prefix, base) = release(pinned)?;
    let mut best: Option<(&String, Vec<u64>)> = None;
    for tag in tags {
        let Some((p, numbers)) = release(tag) else { continue };
        if p != prefix
            || !same_series(&numbers, &base)
            || compare(&numbers, &base) == Ordering::Less
        {
            continue;
        }
        if best.as_ref().is_none_or(|(_, b)| compare(&numbers, b) == Ordering::Greater) {
            best = Some((tag, numbers));
        }
    }
    best.map(|(tag, _)| tag.clone())
}

/// Compares two versions number by number, a missing one counting as 0.
fn compare(a: &[u64], b: &[u64]) -> Ordering {
    for i in 0..a.len().max(b.len()) {
        match a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    Ordering::Equal
}

/// Whether `recipe` is a bundle's port in Rust: one with a Cargo.lock.
fn rust_bundle(recipe: &Recipe) -> bool {
    recipe.bundle.is_some() && !recipe.source.git.is_empty()
}

/// `ob ports audit`: cargo-audit over the Cargo.lock of each bundle's
/// port in Rust, at the commit its recipe pins.
pub fn audit(ctx: &Ctx) -> Result<()> {
    let installed = Command::new("cargo").args(["audit", "--version"]).output();
    if !installed.is_ok_and(|o| o.status.success()) {
        return Err(
            Error::new("cargo-audit is not installed").hint("cargo install cargo-audit --locked")
        );
    }
    let mut failed = Vec::new();
    for (recipe, _) in ports::recipes(ctx)? {
        if !rust_bundle(&recipe) {
            continue;
        }
        let source = &recipe.source;
        let fetched = ports::fetch(ctx, &recipe.name, &source.git, &source.commit, &source.paths)?;
        // As the port builds it: with its patches, which may move crates.
        let dir = ctx.target.join("ports/audit").join(&recipe.name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).context(|| format!("removing {}", dir.display()))?;
        }
        ports::export(ctx, &fetched, &dir, &source.paths)?;
        ports::apply_patches(ctx, &ctx.root.join("ports").join(&recipe.name), &dir)?;
        let lock = dir.join("Cargo.lock");
        if !lock.is_file() {
            continue;
        }
        step(&format!("Auditing the crates of {} {}", recipe.name, recipe.version));
        let mut cmd = Command::new("cargo");
        cmd.args(["audit", "--file"]).arg(&lock);
        if process::run(ctx, &mut cmd).is_err() {
            failed.push(recipe.name.clone());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Error::new(format!("advisories name crates of: {}", failed.join(", ")))
            .hint("move the port to a release whose crates are fixed, or patch them"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(tags: &[&str]) -> Vec<String> {
        tags.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_newest_release_of_the_series_is_found() {
        let tags = list(&["14.1.1", "15.0.0", "15.2.0", "15.10.0-rc1", "15.9.1", "nightly"]);
        assert_eq!(newest("15.2.0", &tags).as_deref(), Some("15.9.1"));
        let tags = list(&["v0.25.0", "v0.26.1", "v0.26.10", "0.99.0"]);
        assert_eq!(newest("v0.26.1", &tags).as_deref(), Some("v0.26.10"));
        let tags = list(&["curl-8_17_0", "curl-8_9_1", "tiny-curl-9_0"]);
        assert_eq!(newest("curl-8_9_1", &tags).as_deref(), Some("curl-8_17_0"));
        assert_eq!(newest("3.5a", &list(&["3.5a", "3.6"])).as_deref(), Some("3.6"));
        assert_eq!(newest("1.0", &list(&["0.9"])), None);
        assert_eq!(newest("2026a", &list(&["2025z", "2026a", "2026f"])).as_deref(), Some("2026f"));
        assert_eq!(newest("v6_6", &list(&["v6_6", "v6_6_20261031"])).as_deref(), Some("v6_6"));
        let tags = list(&["glibc-2.42", "glibc-2.42.9000", "glibc-2.43"]);
        assert_eq!(newest("glibc-2.42", &tags).as_deref(), Some("glibc-2.43"));
    }

    #[test]
    fn versions_compare_by_number() {
        assert_eq!(compare(&[1, 10], &[1, 9]), Ordering::Greater);
        assert_eq!(compare(&[2], &[2, 0]), Ordering::Equal);
        assert_eq!(release("1.0-rc1"), None);
    }
}
