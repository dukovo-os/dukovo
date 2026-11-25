// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The check that only the trusted computing base uses `unsafe` (ADR 0730,
//! docs/coding-standard.md). Every crate of every workspace that is not in
//! [`ALLOWED`] starts each of its targets (its library, programs, tests,
//! benchmarks, examples and build script) with `#![forbid(unsafe_code)]`,
//! which no `allow` inside the crate can undo. A program reaches the C
//! library and the kernel through the safe functions of `sysapi`, which is
//! one of the allowed crates.
//!
//! `ob lint` runs the check before Clippy, from what `cargo metadata` says
//! each workspace holds. When it names a crate, the fix is the forbid line
//! at the top of the file it names, and calls to the C library moved into
//! `sysapi`; a crate joins [`ALLOWED`] only in a reviewed change to the
//! coding standard's list.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::lint::{FUZZ_MANIFEST, MANIFESTS};
use crate::process::{self, step};
use crate::{Ctx, Error, Result};

/// The crates that may use `unsafe`, as docs/coding-standard.md lists
/// them with their reasons: the kernel's core, the user-space runtime and
/// `sysapi`, and the test program that makes raw calls on purpose.
const ALLOWED: &[&str] = &[
    "radix-arch-x86_64",
    "radix-arch-riscv64",
    "radix-sync",
    "radix-hw",
    "radix-mm",
    "radix-uacpi",
    "radix",
    "radix-vdso",
    "libradix",
    "radix-rt",
    "usertests",
    "sysapi",
];

/// The crates that still call the C library themselves. Each moves onto
/// `sysapi` and leaves this list (ADR 0730), which only shrinks: the check
/// fails when a crate here forbids `unsafe` in every target, so that the
/// line goes with the last call.
const NOT_YET: &[&str] = &["nukesh", "lab-writer", "linux-bench"];

/// The line every target of a crate outside [`ALLOWED`] starts with.
const FORBID: &str = "#![forbid(unsafe_code)]";

/// What `cargo metadata --no-deps` says of a workspace.
#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

/// A crate of a workspace.
#[derive(Deserialize)]
struct Package {
    name: String,
    manifest_path: PathBuf,
    targets: Vec<Target>,
}

/// A target of a crate, each a crate of its own to the compiler.
#[derive(Deserialize)]
struct Target {
    /// `lib`, `bin`, `test`, `bench`, `example` or `custom-build`.
    kind: Vec<String>,
    src_path: PathBuf,
}

/// `ob lint`'s check that every crate outside [`ALLOWED`] forbids
/// `unsafe` code.
///
/// # Errors
///
/// When `cargo metadata` fails, a target's file cannot be read, or a
/// crate breaks the rule; the error lists each.
pub fn check(ctx: &Ctx) -> Result<()> {
    step("Checking that only the trusted computing base uses unsafe code");
    // A crate inside another workspace's folder is a member of both, so
    // the crates are kept by manifest.
    let mut packages = BTreeMap::new();
    for manifest in MANIFESTS.iter().chain(&[FUZZ_MANIFEST]) {
        let mut cmd = process::cargo(ctx);
        cmd.args(["metadata", "--no-deps", "--format-version", "1", "--manifest-path", manifest]);
        let output = process::output(ctx, &mut cmd)?;
        let metadata: Metadata = serde_json::from_slice(&output.stdout)
            .map_err(|err| Error::new(format!("cargo metadata for {manifest}: {err}")))?;
        for package in metadata.packages {
            packages.insert(package.manifest_path.clone(), package);
        }
    }
    let mut roots = BTreeMap::new();
    for package in packages.values() {
        for target in &package.targets {
            let source = std::fs::read_to_string(&target.src_path).map_err(|err| {
                Error::new(format!("cannot read {}: {err}", target.src_path.display()))
            })?;
            roots.insert(target.src_path.clone(), forbids(&source));
        }
    }
    let problems = problems(&ctx.root, packages.values(), &roots);
    if problems.is_empty() {
        return Ok(());
    }
    Err(Error::new(format!(
        "crates outside the trusted computing base must not use unsafe code:\n  {}",
        problems.join("\n  ")
    ))
    .hint(format!(
        "start each file named with `{FORBID}`, and call the C library through sysapi's \
         safe functions (docs/coding-standard.md, ADR 0730)"
    )))
}

/// What breaks the rule among `packages`, one line each: a target without
/// the forbid line, a crate of [`NOT_YET`] that no longer needs its place,
/// and a name in either list that no workspace has. `forbidding` says, for
/// each target's file, whether it starts with the forbid line (a file it
/// does not name has none); paths are shown from `root`.
fn problems<'a>(
    root: &Path,
    packages: impl Iterator<Item = &'a Package>,
    forbidding: &BTreeMap<PathBuf, bool>,
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut names = Vec::new();
    for package in packages {
        names.push(package.name.as_str());
        if ALLOWED.contains(&package.name.as_str()) {
            continue;
        }
        let missing: Vec<&Target> = package
            .targets
            .iter()
            .filter(|target| !forbidding.get(&target.src_path).copied().unwrap_or(false))
            .collect();
        if NOT_YET.contains(&package.name.as_str()) {
            if missing.is_empty() {
                problems.push(format!(
                    "{} forbids unsafe code in every target: take it off NOT_YET in \
                     tools/xtask/src/unsafe_code.rs",
                    package.name
                ));
            }
            continue;
        }
        for target in missing {
            let path = target.src_path.strip_prefix(root).unwrap_or(&target.src_path);
            problems.push(format!(
                "{} ({}'s {}) does not start with {FORBID}",
                path.display(),
                package.name,
                target.kind.join(", ")
            ));
        }
    }
    for (list, listed) in [("ALLOWED", ALLOWED), ("NOT_YET", NOT_YET)] {
        for name in listed.iter().filter(|name| !names.contains(name)) {
            problems.push(format!(
                "{list} in tools/xtask/src/unsafe_code.rs names {name}, which no workspace has"
            ));
        }
    }
    problems
}

/// Whether `source`, the file a crate's target starts at, has the forbid
/// line among the inner attributes at its top, after nothing but blank
/// lines, `//` comments and other inner attributes. An inner attribute
/// further down belongs to a module, so it does not count.
fn forbids(source: &str) -> bool {
    // How many brackets an inner attribute that runs over several lines
    // has left open.
    let mut open = 0i32;
    for line in source.lines().map(str::trim) {
        if open > 0 {
            open += brackets(line);
        } else if line == FORBID {
            return true;
        } else if line.starts_with("#![") {
            open = brackets(line);
        } else if !line.is_empty() && !line.starts_with("//") {
            return false;
        }
    }
    false
}

/// The brackets `line` opens, less those it closes.
fn brackets(line: &str) -> i32 {
    line.chars().map(|c| i32::from(c == '[') - i32::from(c == ']')).sum()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use super::{ALLOWED, NOT_YET, Package, Target, forbids, problems};

    fn package(name: &str, sources: &[&str]) -> Package {
        Package {
            name: name.to_string(),
            manifest_path: PathBuf::from(format!("/tree/{name}/Cargo.toml")),
            targets: sources
                .iter()
                .map(|source| Target {
                    kind: vec!["lib".to_string()],
                    src_path: PathBuf::from(format!("/tree/{name}/{source}")),
                })
                .collect(),
        }
    }

    /// Every crate the lists name, so that none is reported missing.
    fn listed() -> Vec<Package> {
        ALLOWED.iter().chain(NOT_YET).map(|name| package(name, &["src/lib.rs"])).collect()
    }

    fn check(packages: &[Package], forbidding: &[(&str, bool)]) -> Vec<String> {
        let forbidding: BTreeMap<PathBuf, bool> =
            forbidding.iter().map(|&(path, forbids)| (PathBuf::from(path), forbids)).collect();
        problems(Path::new("/tree"), packages.iter(), &forbidding)
    }

    #[test]
    fn the_forbid_line_counts_after_comments_and_other_inner_attributes() {
        let source = "// A line comment.\n\n//! A crate.\n\
                      #![no_std]\n#![cfg_attr(\n    not(test),\n    no_main\n)]\n\
                      #![forbid(unsafe_code)]\n\nuse core::fmt;\n";
        assert!(forbids(source));
    }

    #[test]
    fn the_forbid_line_does_not_count_after_an_item_or_inside_a_comment() {
        assert!(!forbids("//! A crate.\n\nuse core::fmt;\n#![forbid(unsafe_code)]\n"));
        assert!(!forbids("//! A crate.\n// #![forbid(unsafe_code)]\nfn main() {}\n"));
        assert!(!forbids("#![deny(unsafe_code)]\n"));
        assert!(!forbids(""));
    }

    #[test]
    fn a_line_inside_a_long_inner_attribute_is_not_the_forbid_line() {
        let source = "#![doc = \"\n#![forbid(unsafe_code)]\n\"]\nfn main() {}\n";
        assert!(!forbids(source));
    }

    #[test]
    fn a_crate_outside_the_lists_needs_the_line_in_every_target() {
        let mut packages = listed();
        packages.push(package("tool", &["src/main.rs", "build.rs"]));
        let problems =
            check(&packages, &[("/tree/tool/src/main.rs", true), ("/tree/tool/build.rs", false)]);
        assert_eq!(
            problems,
            ["tool/build.rs (tool's lib) does not start with #![forbid(unsafe_code)]"]
        );
    }

    #[test]
    fn an_allowed_crate_needs_no_line() {
        assert_eq!(check(&listed(), &[]), Vec::<String>::new());
    }

    #[test]
    fn a_crate_that_no_longer_needs_its_place_on_not_yet_is_named() {
        let name = NOT_YET[0];
        let problems = check(&listed(), &[(&format!("/tree/{name}/src/lib.rs"), true)]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with(&format!("{name} forbids unsafe code in every target")));
    }

    #[test]
    fn a_listed_name_that_no_workspace_has_is_named() {
        let packages: Vec<Package> = listed().into_iter().skip(1).collect();
        let problems = check(&packages, &[]);
        assert_eq!(
            problems,
            [format!(
                "ALLOWED in tools/xtask/src/unsafe_code.rs names {}, which no workspace has",
                ALLOWED[0]
            )]
        );
    }
}
