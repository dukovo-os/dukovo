// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Keeping ports up with their upstreams (ADR 0370).
//!
//! - `ob ports outdated` asks each port's git repository for its tags and
//!   lists the ports whose upstream has released a newer version than the
//!   tag the recipe pins. A port pinned to a tag of git is its own
//!   upstream; a port pinned by a release archive names its project's
//!   repository and the archive's tag in `[upstream]`. CI runs it each
//!   week and opens an issue for each port it lists; a person, who signs
//!   the commit off, moves the pin.
//! - `ob ports audit` checks the Cargo.lock of each bundle's port in Rust,
//!   with the port's patches, against the RustSec advisory database with
//!   cargo-audit, so that a flaw in a crate a bundle links is known the day
//!   it is published. A vulnerability fails it; a crate that is only
//!   unmaintained or unsound is shown, not failed on, since a port often
//!   cannot leave it. CI runs `ob security scan` each day instead, which
//!   audits these locks with every other Cargo.lock of the system, and
//!   takes the locks from [`bundle_locks`], with the go.mod of each
//!   bundle's port in Go.

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::path::PathBuf;
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

/// `ob ports outdated`: the ports whose upstream has a newer release than
/// the one the recipe pins. A port pinned to a tag of git is checked
/// against its own repository, and a port pinned by a release archive
/// against the repository its `[upstream]` names, by the tag it names. With
/// `tsv`, one `name\tpinned\tnewer` line each, for CI; else a table, with
/// every port it checked. The toolchain's ports, the ports pinned to a
/// commit alone and those whose `[upstream]` says `none` are not checked.
pub fn outdated(ctx: &Ctx, tsv: bool) -> Result<()> {
    let recipes: Vec<Recipe> = ports::recipes(ctx)?.into_iter().map(|(recipe, _)| recipe).collect();
    let (found, failed) = survey(&recipes, &|git| tags(ctx, git));
    print!("{}", render(&found, tsv));
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Error::new(format!("could not ask upstream of:\n  {}", failed.join("\n  "))))
    }
}

/// The repository and the tag to read a port's releases by: the port's own
/// source when it is pinned to a tag of git, else the `[upstream]` of a
/// port pinned by a release archive.
fn tracked(recipe: &Recipe) -> Option<(&str, &str)> {
    let source = &recipe.source;
    if !source.git.is_empty() {
        return source.tag.as_deref().map(|tag| (source.git.as_str(), tag));
    }
    recipe.upstream.as_ref()?.tagged()
}

/// What `recipes` have of newer releases, as `tags_of` lists the tags of a
/// repository: each port it could ask, and a line for each port it could
/// not, which says why.
fn survey(
    recipes: &[Recipe],
    tags_of: &dyn Fn(&str) -> Result<Vec<String>>,
) -> (Vec<Found>, Vec<String>) {
    let mut found = Vec::new();
    let mut failed = Vec::new();
    for recipe in recipes {
        let Some((git, tag)) = tracked(recipe) else { continue };
        match tags_of(git) {
            Ok(tags) => found.push(Found {
                newer: newest(tag, &tags, recipe.series.as_deref()).filter(|newest| newest != tag),
                name: recipe.name.clone(),
                pinned: tag.to_string(),
            }),
            Err(err) => failed.push(format!("{}: {err}", recipe.name)),
        }
    }
    (found, failed)
}

/// The list `ob ports outdated` prints: the ports with a newer release as
/// tab-separated lines with `tsv`, else every port in a table.
fn render(found: &[Found], tsv: bool) -> String {
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
        let name_width = found.iter().map(|p| p.name.len()).max().unwrap_or(0);
        let tag_width = found.iter().map(|p| p.pinned.len()).max().unwrap_or(0);
        for port in found {
            let state =
                port.newer.as_ref().map_or("up to date".to_string(), |n| format!("{n} is out"));
            let _ = writeln!(out, "{:name_width$}  {:tag_width$}  {state}", port.name, port.pinned);
        }
    }
    out
}

/// The tags of the repository `git`, by name.
fn tags(ctx: &Ctx, git: &str) -> Result<Vec<String>> {
    let output = ls_remote(ctx, git, &["--tags", "--refs"], &[])?;
    Ok(output
        .lines()
        .filter_map(|line| line.split('\t').nth(1)?.strip_prefix("refs/tags/").map(str::to_string))
        .collect())
}

/// How many times `git ls-remote` is tried before a repository counts as
/// out of reach: a host that drops one connection should not fail the
/// day's scan.
const LS_REMOTE_TRIES: u32 = 3;

/// What `git ls-remote <options> <git> <patterns>` prints, tried up to
/// [`LS_REMOTE_TRIES`] times, 2 and then 4 seconds apart. It never asks
/// for a password and gives up on a connection that has stopped.
pub fn ls_remote(ctx: &Ctx, git: &str, options: &[&str], patterns: &[String]) -> Result<String> {
    let mut tried = 0;
    loop {
        let mut cmd = Command::new("git");
        cmd.args(["-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=60"]);
        cmd.arg("ls-remote").args(options).arg(git).args(patterns);
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        match process::output(ctx, &mut cmd) {
            Ok(output) => return Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
            Err(err) => {
                tried += 1;
                if tried == LS_REMOTE_TRIES {
                    return Err(err);
                }
                std::thread::sleep(std::time::Duration::from_secs(2 << (tried - 1)));
            }
        }
    }
}

/// A release tag's numbers, and what comes before them (such as `v` or
/// `curl-`), which a newer tag of the same series shares; none for a tag
/// that is not a plain release, such as `1.0-rc1` or `nightly`. A letter
/// after the numbers, as in tzdata's 2026f, counts as one more number.
/// The numbers start after a separator, or after the letters of the
/// project's name, as in v668 and FILE5_47, so that a name with a digit in
/// it, as in bzip2-1.0.8, is part of the prefix. libevent's `-stable` after
/// its stable releases, as in release-2.1.13-stable, is not part of the
/// numbers; its `-alpha` and `-rc` tags are no plain release.
fn release(tag: &str) -> Option<(&str, Vec<u64>)> {
    let tag = tag.strip_suffix("-stable").unwrap_or(tag);
    let mut previous: Option<char> = None;
    let mut digit_seen = false;
    for (at, c) in tag.char_indices() {
        let starts_number = c.is_ascii_digit() && previous.is_none_or(|p| !p.is_ascii_digit());
        let after_name = previous
            .is_none_or(|p| !p.is_ascii_alphanumeric() || (p.is_ascii_alphabetic() && !digit_seen));
        if starts_number
            && after_name
            && let Some(numbers) = release_numbers(&tag[at..])
        {
            return Some((&tag[..at], numbers));
        }
        digit_seen |= c.is_ascii_digit();
        previous = Some(c);
    }
    None
}

/// The numbers of `text`, which is only a release's version: numbers
/// separated by `.` or `_`, and perhaps a letter.
fn release_numbers(text: &str) -> Option<Vec<u64>> {
    let (text, letter) = match text.strip_suffix(|c: char| c.is_ascii_lowercase()) {
        Some(numbers) => (numbers, text.bytes().last().map(|b| u64::from(b - b'a' + 1))),
        None => (text, None),
    };
    let numbers: Option<Vec<u64>> = text.split(['.', '_']).map(|part| part.parse().ok()).collect();
    let mut numbers = numbers.filter(|n| !n.is_empty())?;
    numbers.extend(letter);
    Some(numbers)
}

/// Whether `numbers` are a release in the series of `base`: any numbers
/// beyond base's are small, as in 1.2.5.1; not a snapshot's date
/// (ncurses' 6_6_20261031) or a development version (glibc's 2.42.9000).
fn same_series(numbers: &[u64], base: &[u64]) -> bool {
    numbers.iter().skip(base.len()).all(|&n| n < 1000)
}

/// The newest of `tags` in `pinned`'s series, if any tag is newer than or
/// equal to it. With `follows`, such as `3.5`, only a tag whose leading
/// numbers are those counts.
fn newest(pinned: &str, tags: &[String], follows: Option<&str>) -> Option<String> {
    let (prefix, base) = release(pinned)?;
    let follows: Vec<u64> = match follows {
        Some(series) => series.split('.').map(str::parse).collect::<Result<_, _>>().ok()?,
        None => Vec::new(),
    };
    let mut best: Option<(&String, Vec<u64>)> = None;
    for tag in tags {
        let Some((p, numbers)) = release(tag) else { continue };
        if p != prefix
            || !same_series(&numbers, &base)
            || !numbers.starts_with(&follows)
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

/// Whether `recipe` is a bundle's port from git, whose source may pin
/// what it builds with in a Cargo.lock or a go.mod.
fn git_bundle(recipe: &Recipe) -> bool {
    recipe.bundle.is_some() && !recipe.source.git.is_empty()
}

/// What pins the modules or crates a bundle's port builds with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockKind {
    /// A Cargo.lock: the port is in Rust.
    Cargo,
    /// A go.mod, whose modules go.sum pins by hash: the port is in Go.
    GoMod,
}

/// The Cargo.lock or go.mod of a bundle's port.
pub struct PortLock {
    /// The port's name.
    pub port: String,
    /// The port's version.
    pub version: String,
    /// The file, in the port's source as it builds.
    pub lock: PathBuf,
    /// Which file it is.
    pub kind: LockKind,
}

/// Fails, with the command that installs it, unless cargo-audit is.
pub fn require_cargo_audit() -> Result<()> {
    let installed = Command::new("cargo").args(["audit", "--version"]).output();
    if installed.is_ok_and(|o| o.status.success()) {
        Ok(())
    } else {
        Err(Error::new("cargo-audit is not installed").hint("cargo install cargo-audit --locked"))
    }
}

/// The Cargo.lock of each bundle's port in Rust, and the go.mod of each in
/// Go, from the port's source at the commit its recipe pins, fetched, with
/// the port's patches applied since they may move crates or modules, as
/// the port builds. The source is exported into `ports/audit/<port>` of
/// the target folder. A bundle whose source has neither is left out.
pub fn bundle_locks(ctx: &Ctx) -> Result<Vec<PortLock>> {
    let mut locks = Vec::new();
    for (recipe, _) in ports::recipes(ctx)? {
        if !git_bundle(&recipe) {
            continue;
        }
        let source = &recipe.source;
        let fetched = ports::fetch(ctx, &recipe.name, &source.git, &source.commit, &source.paths)?;
        let dir = ctx.target.join("ports/audit").join(&recipe.name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).context(|| format!("removing {}", dir.display()))?;
        }
        ports::export(ctx, &fetched, &dir, &source.paths)?;
        ports::apply_patches(ctx, &ctx.root.join("ports").join(&recipe.name), &dir)?;
        for (file, kind) in [("Cargo.lock", LockKind::Cargo), ("go.mod", LockKind::GoMod)] {
            let lock = dir.join(file);
            if lock.is_file() {
                let (port, version) = (recipe.name.clone(), recipe.version.clone());
                locks.push(PortLock { port, version, lock, kind });
            }
        }
    }
    Ok(locks)
}

/// `ob ports audit`: cargo-audit over the Cargo.lock of each bundle's
/// port in Rust, at the commit its recipe pins.
pub fn audit(ctx: &Ctx) -> Result<()> {
    require_cargo_audit()?;
    let mut failed = Vec::new();
    for found in bundle_locks(ctx)?.into_iter().filter(|found| found.kind == LockKind::Cargo) {
        step(&format!("Auditing the crates of {} {}", found.port, found.version));
        let mut cmd = Command::new("cargo");
        cmd.args(["audit", "--file"]).arg(&found.lock);
        if process::run(ctx, &mut cmd).is_err() {
            failed.push(found.port);
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
        assert_eq!(newest("15.2.0", &tags, None).as_deref(), Some("15.9.1"));
        let tags = list(&["v0.25.0", "v0.26.1", "v0.26.10", "0.99.0"]);
        assert_eq!(newest("v0.26.1", &tags, None).as_deref(), Some("v0.26.10"));
        let tags = list(&["curl-8_17_0", "curl-8_9_1", "tiny-curl-9_0"]);
        assert_eq!(newest("curl-8_9_1", &tags, None).as_deref(), Some("curl-8_17_0"));
        assert_eq!(newest("3.5a", &list(&["3.5a", "3.6"]), None).as_deref(), Some("3.6"));
        assert_eq!(newest("1.0", &list(&["0.9"]), None), None);
        // A port that follows a series is offered only that series'
        // releases: OpenSSL 3.5, a long-term release, is not moved to 4.
        let openssl = list(&["openssl-3.5.4", "openssl-3.5.6", "openssl-3.6.1", "openssl-4.1.0"]);
        assert_eq!(newest("openssl-3.5.4", &openssl, None).as_deref(), Some("openssl-4.1.0"));
        assert_eq!(
            newest("openssl-3.5.4", &openssl, Some("3.5")).as_deref(),
            Some("openssl-3.5.6")
        );
        assert_eq!(
            newest("openssl-3.5.4", &openssl[..1], Some("3.5")).as_deref(),
            Some("openssl-3.5.4")
        );
        assert_eq!(newest("openssl-3.5.4", &openssl, Some("3")).as_deref(), Some("openssl-3.6.1"));
        assert_eq!(
            newest("2026a", &list(&["2025z", "2026a", "2026f"]), None).as_deref(),
            Some("2026f")
        );
        assert_eq!(
            newest("v6_6", &list(&["v6_6", "v6_6_20261031"]), None).as_deref(),
            Some("v6_6")
        );
        let tags = list(&["glibc-2.42", "glibc-2.42.9000", "glibc-2.43"]);
        assert_eq!(newest("glibc-2.42", &tags, None).as_deref(), Some("glibc-2.43"));
    }

    #[test]
    fn libevent_stable_releases_are_releases_and_its_alphas_are_not() {
        let tags = list(&[
            "release-2.1.12-stable",
            "release-2.1.13-stable",
            "release-2.2.1-alpha",
            "release-2.2.2-alpha",
            "release-2.1.7-rc",
        ]);
        assert_eq!(release("release-2.1.13-stable"), Some(("release-", vec![2, 1, 13])));
        assert_eq!(release("release-2.2.1-alpha"), None);
        assert_eq!(
            newest("release-2.1.13-stable", &tags, None).as_deref(),
            Some("release-2.1.13-stable")
        );
        assert_eq!(
            newest("release-2.1.12-stable", &tags, None).as_deref(),
            Some("release-2.1.13-stable")
        );
        let later = list(&["release-2.1.13-stable", "release-2.1.14-stable"]);
        assert_eq!(
            newest("release-2.1.13-stable", &later, None).as_deref(),
            Some("release-2.1.14-stable")
        );
    }

    #[test]
    fn the_numbers_of_a_tag_start_after_a_separator_or_the_name_and_a_candidate_has_none() {
        assert_eq!(release("bzip2-1.0.8"), Some(("bzip2-", vec![1, 0, 8])));
        assert_eq!(release("FILE5_47"), Some(("FILE", vec![5, 47])));
        assert_eq!(release("v668"), Some(("v", vec![668])));
        // A candidate or a snapshot is no release, though a number ends its tag.
        for tag in ["jq-1.8.2rc1", "1.0-rc1", "v710-rel", "nightly"] {
            assert_eq!(release(tag), None, "{tag}");
        }
        let bzip2 = list(&["bzip2-1.0.6", "bzip2-1.0.8", "bzip2-1.0.8-rc1", "bzip2-1.1.0"]);
        assert_eq!(newest("bzip2-1.0.6", &bzip2, None).as_deref(), Some("bzip2-1.1.0"));
        assert_eq!(newest("bzip2-1.0.8", &bzip2[..3], None).as_deref(), Some("bzip2-1.0.8"));
    }

    #[test]
    fn the_tags_of_archive_ports_are_compared_by_the_series_rules() {
        // Tags as the projects' repositories list them.
        let file = list(&["FILE5_46", "FILE5_47", "FILE5_48", "pre-rrt-big-changes-post-4-23"]);
        assert_eq!(newest("FILE5_47", &file, None).as_deref(), Some("FILE5_48"));
        let tmux = list(&["3.7", "3.7b", "3.7c", "3.8", "3.8-rc"]);
        assert_eq!(newest("3.7c", &tmux, None).as_deref(), Some("3.8"));
        let jq = list(&["jq-1.8.1", "jq-1.8.2", "jq-1.8.2rc1"]);
        assert_eq!(newest("jq-1.8.2", &jq, None).as_deref(), Some("jq-1.8.2"));
        let zip = list(&["25.01", "26.02", "26.06"]);
        assert_eq!(newest("26.02", &zip, None).as_deref(), Some("26.06"));
        let less = list(&["v668", "v710", "v710-rel", "v712"]);
        assert_eq!(newest("v668", &less, None).as_deref(), Some("v712"));
        // A tag the numbers cannot be read from is not compared, and the
        // port is then never listed as behind.
        assert_eq!(newest("nightly", &less, None), None);
    }

    fn recipe(text: &str) -> Recipe {
        let header =
            "version = \"1\"\nsummary = \"s\"\nlicense = \"MIT\"\nhomepage = \"https://x\"\n";
        toml::from_str(&format!("{header}{text}")).unwrap()
    }

    fn recipes() -> Vec<Recipe> {
        let sha = "ab".repeat(32);
        let commit = "0123456789012345678901234567890123456789";
        [
            format!(
                "name = \"lua\"\n[source]\ngit = \"https://x.org/lua.git\"\ncommit = \"{commit}\"\n\
                 tag = \"v5.4.7\""
            ),
            format!(
                "name = \"curl\"\n[source]\nurl = \"https://x.org/curl.tar.xz\"\nsha256 = \"{sha}\"\n\
                 [upstream]\ngit = \"https://x.org/curl.git\"\ntag = \"curl-8_9_1\""
            ),
            format!(
                "name = \"zip\"\n[source]\nurl = \"https://x.org/zip.tar.gz\"\nsha256 = \"{sha}\"\n\
                 [upstream]\nnone = \"no repository\""
            ),
            // Pinned to a commit alone, and with no source of its own.
            format!(
                "name = \"loose\"\n[source]\ngit = \"https://x.org/loose.git\"\ncommit = \"{commit}\""
            ),
            "name = \"crates\"\n[source]\nrust-src = true".to_string(),
        ]
        .iter()
        .map(|text| recipe(text))
        .collect()
    }

    #[test]
    fn a_port_pinned_by_an_archive_is_read_through_its_upstream() {
        let recipes = recipes();
        let tracked: Vec<_> = recipes.iter().map(tracked).collect();
        assert_eq!(
            tracked,
            [
                Some(("https://x.org/lua.git", "v5.4.7")),
                Some(("https://x.org/curl.git", "curl-8_9_1")),
                None,
                None,
                None,
            ]
        );

        let asked = std::cell::RefCell::new(Vec::new());
        let tags_of = |git: &str| -> Result<Vec<String>> {
            asked.borrow_mut().push(git.to_string());
            Ok(match git {
                "https://x.org/lua.git" => list(&["v5.4.6", "v5.4.7"]),
                _ => list(&["curl-8_9_1", "curl-8_17_0", "curl-8_9_0"]),
            })
        };
        let (found, failed) = survey(&recipes, &tags_of);
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(
            found,
            [
                Found { name: "lua".into(), pinned: "v5.4.7".into(), newer: None },
                Found {
                    name: "curl".into(),
                    pinned: "curl-8_9_1".into(),
                    newer: Some("curl-8_17_0".into())
                },
            ]
        );
        // One question for each port that has a repository, and none for the rest.
        assert_eq!(*asked.borrow(), ["https://x.org/lua.git", "https://x.org/curl.git"]);
    }

    #[test]
    fn a_repository_that_cannot_be_asked_names_its_port() {
        let recipes = recipes();
        let tags_of = |git: &str| -> Result<Vec<String>> {
            if git.ends_with("curl.git") {
                Err(Error::new("unable to access the repository"))
            } else {
                Ok(list(&["v5.4.7"]))
            }
        };
        let (found, failed) = survey(&recipes, &tags_of);
        assert_eq!(found.len(), 1);
        assert_eq!(failed, ["curl: unable to access the repository"]);
    }

    #[test]
    fn the_list_is_a_table_of_every_port_or_the_tab_separated_ports_that_are_behind() {
        let found = [
            Found { name: "lua".into(), pinned: "v5.4.7".into(), newer: None },
            Found {
                name: "libevent".into(),
                pinned: "release-2.1.13-stable".into(),
                newer: Some("release-2.1.14-stable".into()),
            },
        ];
        assert_eq!(
            render(&found, false),
            "lua       v5.4.7                 up to date\n\
             libevent  release-2.1.13-stable  release-2.1.14-stable is out\n"
        );
        assert_eq!(
            render(&found, true),
            "libevent\trelease-2.1.13-stable\trelease-2.1.14-stable\n"
        );
        assert_eq!(render(&[], true), "");
    }

    /// A pinned tag that `release` cannot read is compared with nothing,
    /// and its port is listed as up to date whatever upstream does.
    #[test]
    fn every_tag_an_archive_port_is_checked_by_reads_as_a_release() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let ctx = Ctx {
            out: root.join("out"),
            target: root.join("target"),
            root,
            config: crate::config::Config::default(),
            verbose: false,
        };
        let mut checked = 0;
        for (recipe, _) in ports::recipes(&ctx).unwrap() {
            let Some((_, tag)) = recipe.upstream.as_ref().and_then(ports::Upstream::tagged) else {
                continue;
            };
            assert!(release(tag).is_some(), "{}: `{tag}` is no release", recipe.name);
            checked += 1;
        }
        assert!(checked >= 20, "only {checked} archive ports are checked");
    }

    #[test]
    fn versions_compare_by_number() {
        assert_eq!(compare(&[1, 10], &[1, 9]), Ordering::Greater);
        assert_eq!(compare(&[2], &[2, 0]), Ordering::Equal);
        assert_eq!(release("1.0-rc1"), None);
    }
}
