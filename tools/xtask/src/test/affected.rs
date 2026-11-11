// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob test boot --affected <commit>`: the boot tests that the changes
//! since a commit can reach (ADR 0421), which a pull request runs in
//! place of all of them. Main runs every boot test after each merge and
//! every night, so a test a pull request leaves out still runs within
//! hours.
//!
//! Each changed file counts in one of four ways, the first that fits:
//!
//! - It is in [`NOTHING`]: no boot test reads it (writing for people, the
//!   lab's tests, other jobs' tools). It adds no test.
//! - It is a test's own file, `test/cases/<name>.rs`: that test.
//! - Tests name it in `test/covers.txt`: those tests, and the broad
//!   [`SAMPLE`] too when it is in [`CORE`], what every boot stands on.
//! - No test names it: the [`SAMPLE`], since anything could use it.
//!
//! Whenever a change adds any test, the [`SMOKE`] tests run as well, and
//! so does every test that `test/covers.txt` does not name, as nothing
//! says what it covers.

use std::collections::BTreeSet;
use std::process::Command;

use super::{Case, all_cases, cases_seconds};
use crate::process;
use crate::{Ctx, Result};

/// Paths that no boot test reads, so changing them runs none. A path that
/// ends in `/` is a folder; one that starts with `*` matches any path
/// ending so (`*.md`), or any folder of that name (`*/tla/`).
const NOTHING: &[&str] = &[
    // Writing for people: no build reads Markdown. The manual pages are
    // not here, as they go into the system.
    "*.md",
    "docs/adr/",
    "docs/design/",
    "docs/handbook/",
    "docs/images/",
    "credits/",
    "LICENSES/",
    "LICENSE-APACHE",
    "LICENSE-MIT",
    "DCO",
    "REUSE.toml",
    // The TLA+ models, which their own job checks.
    "*/tla/",
    // The Dukovo Lab's tests and its part of ob, run on the lab's
    // machines.
    "lab/",
    "tools/xtask/src/lab.rs",
    // Other jobs' tools and checks.
    "deny.toml",
    "kernel/deny.toml",
    "rustfmt.toml",
    "tools/container/",
    "tools/windows/",
    "tools/update-server/",
    "tools/rge-openbsd-check/",
    "tools/yubikey-piv.py",
    "tools/xtask/src/baseline.rs",
    "tools/xtask/src/corpus.rs",
    "tools/xtask/src/riscv.rs",
    "tools/xtask/src/scaling.rs",
    "tools/xtask/src/shellbench.rs",
    "tools/xtask/src/signoff.rs",
    "tools/xtask/src/tla.rs",
    "tools/xtask/src/tuf.rs",
    "tools/xtask/src/upstream.rs",
    // How long the tests take and what they cover choose and share out
    // the tests; they change no test.
    "tools/xtask/src/test/times.txt",
    "tools/xtask/src/test/covers.txt",
];

/// What every boot stands on: the kernel, the system call interface, the
/// C library and the Rust runtime, Spark and the system's defaults, the
/// test harness and the image it boots, the toolchain, and CI. A change
/// here runs the [`SAMPLE`] as well as the tests that name the path.
const CORE: &[&str] = &[
    "kernel/",
    "abi/",
    "tools/ridlc/",
    "ports/mlibc/",
    "ports/freestnd-cxx-hdrs/",
    "ports/llvm-runtimes/",
    "ports/rust-std/",
    "ports/rust-crates/",
    "userspace/spark/",
    "userspace/libradix/",
    "userspace/radix-rt/",
    "userspace/defaults/",
    "userspace/Cargo.toml",
    "userspace/Cargo.lock",
    "userspace/link.ld",
    "userspace/targets/",
    "userspace/programs/Cargo.toml",
    "userspace/programs/Cargo.lock",
    "userspace/programs/systools/src/main.rs",
    "tools/xtask/",
    "image/",
    "ob",
    "dukovo.toml",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".github/",
];

/// Where each boot test's own file is: a change to it runs that test.
const CASE_FILES: &str = "tools/xtask/src/test/cases/";

/// The broad sample a change to the [`CORE`] runs, about 14 minutes on
/// CI: each way of booting, the kernel's self-tests, Spark and the
/// services, the console, the C library's and the commands' suite, a
/// filesystem of each kind, the network, accounts, USB, sign-in over SSH
/// and TLS, and an installed system.
pub const SAMPLE: &[&str] = &[
    // Booting.
    "uefi-usb",
    "bios-usb",
    "uefi-cdrom",
    "bios-cdrom",
    // The kernel's self-tests.
    "page-fault",
    "kdb",
    "mm-stress",
    "tlb",
    "caps",
    "stack-overflow",
    "smep",
    "fpu",
    "sched-fair",
    "sched-pi",
    "lockdep",
    "rcu",
    "user",
    "ipc",
    "fuzz",
    "oom",
    "net",
    // Spark, the services and the console.
    "spark",
    "services",
    "logging",
    "consoles",
    // The C library and every command.
    "posix",
    // Disks and filesystems.
    "storage",
    "storage-nvme",
    "btrfs-write",
    "ext4-write",
    "ntfs",
    // The network.
    "sockets",
    "dhcp",
    "ssh",
    "tls",
    // Accounts, USB and the installed system.
    "accounts",
    "usb-input",
    "installed-uefi",
];

/// The tests every selection holds, whatever changed: a boot under each
/// firmware, and Spark starting a program.
pub const SMOKE: &[&str] = &["uefi-usb", "bios-cdrom", "spark"];

/// The paths each boot test covers, from test/covers.txt.
const COVERS: &str = include_str!("covers.txt");

/// The lines of test/covers.txt, as (test, path) pairs.
fn covers() -> impl Iterator<Item = (&'static str, &'static str)> {
    COVERS.lines().filter(|line| !line.is_empty() && !line.starts_with('#')).flat_map(|line| {
        let mut words = line.split_whitespace();
        let name = words.next().unwrap_or_default();
        words.map(move |path| (name, path))
    })
}

/// Whether `pattern`, as [`NOTHING`] and test/covers.txt write paths,
/// matches the file `path`.
fn matches(pattern: &str, path: &str) -> bool {
    if let Some(rest) = pattern.strip_prefix('*') {
        if rest.ends_with('/') {
            return path.contains(rest) || path.starts_with(&rest[1..]);
        }
        return path.ends_with(rest);
    }
    if pattern.ends_with('/') { path.starts_with(pattern) } else { path == pattern }
}

fn matches_any(patterns: &[&str], path: &str) -> bool {
    patterns.iter().any(|pattern| matches(pattern, path))
}

/// The boot tests a set of changed files reaches, and why.
pub struct Choice {
    /// The tests, in the order they run.
    pub cases: Vec<&'static Case>,
    /// Whether a change reached the [`SAMPLE`].
    pub sample: bool,
    /// One line for each changed file: what it adds.
    pub reasons: Vec<String>,
}

/// The boot test that runs the examples the docs mark as tests (ADR 0420).
pub const EXAMPLES: &str = "docs-examples";

/// The boot tests that changes to `changed`, paths from the repository's
/// root, reach. `has_examples` says whether a changed file holds examples
/// marked as tests, which run [`EXAMPLES`] although no build reads the
/// file; a guide, tutorial or runbook runs its own lab test likewise.
pub fn choose(changed: &[String], has_examples: &dyn Fn(&str) -> bool) -> Choice {
    let mut chosen: BTreeSet<&str> = BTreeSet::new();
    let mut sample = false;
    let mut any = false;
    let mut reasons = Vec::new();
    for path in changed {
        let mut docs: Vec<&'static str> = Vec::new();
        if crate::site::check::example_file(path) && has_examples(path) {
            docs.push(EXAMPLES);
        }
        if let Some(test) = crate::site::check::page_test(path)
            && let Some(case) = all_cases().find(|case| case.name == test)
        {
            docs.push(case.name);
        }
        if !docs.is_empty() {
            any = true;
            chosen.extend(&docs);
            reasons.push(format!("{path}: {}", docs.join(", ")));
            continue;
        }
        if matches_any(NOTHING, path) {
            reasons.push(format!("{path}: no boot test"));
            continue;
        }
        any = true;
        let own_test = path
            .strip_prefix(CASE_FILES)
            .and_then(|file| file.strip_suffix(".rs"))
            .and_then(|name| all_cases().find(|case| case.name == name));
        if let Some(case) = own_test {
            chosen.insert(case.name);
            reasons.push(format!("{path}: {}", case.name));
            continue;
        }
        let covering: Vec<&str> =
            covers().filter(|&(_, pattern)| matches(pattern, path)).map(|(name, _)| name).collect();
        let covering: BTreeSet<&str> = covering.into_iter().collect();
        chosen.extend(&covering);
        let names = covering.iter().copied().collect::<Vec<_>>().join(", ");
        if matches_any(CORE, path) {
            sample = true;
            if covering.is_empty() {
                reasons.push(format!("{path}: the sample"));
            } else {
                reasons.push(format!("{path}: the sample, and {names}"));
            }
        } else if covering.is_empty() {
            sample = true;
            reasons.push(format!("{path}: the sample (no test names it in test/covers.txt)"));
        } else {
            reasons.push(format!("{path}: {names}"));
        }
    }
    if any {
        chosen.extend(SMOKE);
        if sample {
            chosen.extend(SAMPLE);
        }
        chosen.extend(unmapped());
    }
    let cases = all_cases().filter(|case| chosen.contains(case.name)).collect();
    Choice { cases, sample, reasons }
}

/// The boot tests that test/covers.txt does not name, which run whenever
/// any boot test does.
pub fn unmapped() -> Vec<&'static str> {
    all_cases()
        .map(|case| case.name)
        .filter(|&name| !covers().any(|(covered, _)| covered == name))
        .collect()
}

/// The files changed since where HEAD and `commit` parted, the working
/// tree's changes and new files included, from the repository's root.
pub fn changed_files(ctx: &Ctx, commit: &str) -> Result<Vec<String>> {
    let git = |args: &[&str]| -> Result<Vec<String>> {
        let mut cmd = Command::new("git");
        cmd.current_dir(&ctx.root).args(args);
        let output = process::output(ctx, &mut cmd).map_err(|err| {
            err.hint(format!(
                "--affected needs `{commit}` and the commit where HEAD left it in this clone; \
                 fetch them first"
            ))
        })?;
        Ok(String::from_utf8_lossy(&output.stdout).lines().map(str::to_string).collect())
    };
    // Without renames, so that a file moved away counts where it was too.
    let mut files = git(&["diff", "--name-only", "--no-renames", "--merge-base", commit])?;
    files.extend(git(&["ls-files", "--others", "--exclude-standard"])?);
    files.sort_unstable();
    files.dedup();
    Ok(files)
}

/// How long CI's machines may spend on a share of the chosen tests, in
/// seconds, before [`parts`] splits them between more machines.
const SHARE_SECONDS: u32 = 600;

/// The most machines CI splits the boot tests between, as for all of them.
pub const MOST_PARTS: usize = 5;

/// How many of CI's machines `cases` need: none for no test, else one for
/// each [`SHARE_SECONDS`] of tests by test/times.txt, up to [`MOST_PARTS`].
pub fn parts(cases: &[&Case]) -> usize {
    if cases.is_empty() {
        return 0;
    }
    let seconds = cases_seconds(cases);
    (seconds.div_ceil(SHARE_SECONDS) as usize).clamp(1, MOST_PARTS)
}

/// `--plan`: the number of machines for CI's boot jobs, as `parts=<n>`,
/// and their matrix, as `shards=[1,...]` (`[0]` for none, a job the
/// workflow skips), for $GITHUB_OUTPUT.
pub fn plan(cases: &[&Case]) -> String {
    let parts = parts(cases);
    let shards: Vec<String> = if parts == 0 {
        vec!["0".to_string()]
    } else {
        (1..=parts).map(|part| part.to_string()).collect()
    };
    format!("parts={parts}\nshards=[{}]\n", shards.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn choose_for(paths: &[&str]) -> Choice {
        choose(&paths.iter().map(ToString::to_string).collect::<Vec<_>>(), &|_| false)
    }

    fn names(choice: &Choice) -> Vec<&'static str> {
        choice.cases.iter().map(|case| case.name).collect()
    }

    #[test]
    fn covers_names_real_tests_and_paths() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut lines = 0;
        for (name, path) in covers() {
            lines += 1;
            assert!(all_cases().any(|case| case.name == name), "test/covers.txt: no test `{name}`");
            assert!(!path.starts_with('*'), "test/covers.txt: `{path}` is not a path");
            assert!(
                root.join(path.trim_end_matches('/')).exists(),
                "test/covers.txt: `{name}` covers `{path}`, which is not in the tree"
            );
            assert!(
                !matches_any(NOTHING, path.trim_end_matches('/')),
                "test/covers.txt: `{path}` is a path no boot test reads"
            );
        }
        assert!(lines > 0);
    }

    #[test]
    fn sample_and_smoke_name_real_tests() {
        for name in SAMPLE.iter().chain(SMOKE) {
            assert!(all_cases().any(|case| case.name == *name), "no test `{name}`");
        }
        assert!(SMOKE.iter().all(|name| SAMPLE.contains(name)), "the sample holds the smoke tests");
        let sample: Vec<&Case> = all_cases().filter(|case| SAMPLE.contains(&case.name)).collect();
        // The sample fits two of CI's machines.
        assert!(cases_seconds(&sample) <= 2 * SHARE_SECONDS, "{} s", cases_seconds(&sample));
    }

    #[test]
    fn paths_match_as_written() {
        assert!(matches("*.md", "docs/roadmap.md"));
        assert!(matches("*.md", "README.md"));
        assert!(!matches("*.md", "docs/man/ob.1.scd"));
        assert!(matches("*/tla/", "kernel/cap/tla/Capabilities.tla"));
        assert!(matches("*/tla/", "tla/x.tla"));
        assert!(!matches("*/tla/", "tools/xtask/src/tla.rs"));
        assert!(matches("kernel/", "kernel/radix/src/main.rs"));
        assert!(!matches("kernel/", "kernelx/a"));
        assert!(matches("ob", "ob"));
        assert!(!matches("ob", "obx"));
    }

    #[test]
    fn writing_and_lab_tests_run_no_boot_test() {
        let choice = choose_for(&[
            "docs/adr/0421-x.md",
            "docs/handbook/src/a.md",
            "lab/tests/network/a.toml",
            "kernel/cap/tla/Capabilities.tla",
            "tools/xtask/src/lab.rs",
        ]);
        assert!(choice.cases.is_empty());
        assert!(!choice.sample);
        assert_eq!(parts(&choice.cases), 0);
        assert_eq!(plan(&choice.cases), "parts=0\nshards=[0]\n");
    }

    #[test]
    fn docs_with_examples_run_the_examples_test() {
        let changed = ["docs/handbook/src/a.md".to_string(), "docs/adr/0421-x.md".to_string()];
        let changed = [changed.to_vec(), vec!["CONTRIBUTING.md".to_string()]].concat();
        let choice =
            choose(&changed, &|path| path == "docs/handbook/src/a.md" || path == "CONTRIBUTING.md");
        let chosen = names(&choice);
        assert!(!choice.sample);
        assert!(chosen.contains(&EXAMPLES), "{chosen:?}");
        assert!(SMOKE.iter().all(|name| chosen.contains(name)), "{chosen:?}");
        assert_eq!(choice.reasons[0], "docs/handbook/src/a.md: docs-examples");
        assert_eq!(choice.reasons[1], "docs/adr/0421-x.md: no boot test");
        // Examples run only from the folders the test reads.
        assert_eq!(choice.reasons[2], "CONTRIBUTING.md: no boot test");
    }

    #[test]
    fn a_program_runs_its_tests_and_the_smoke_tests() {
        let choice = choose_for(&["userspace/programs/installer/src/plan.rs"]);
        let chosen = names(&choice);
        assert!(!choice.sample);
        assert!(chosen.contains(&"install-uefi"), "{chosen:?}");
        assert!(SMOKE.iter().all(|name| chosen.contains(name)), "{chosen:?}");
        assert!(!chosen.contains(&"btrfs-stress"), "{chosen:?}");
        assert!(chosen.len() < SAMPLE.len(), "{chosen:?}");
    }

    #[test]
    fn a_test_file_runs_that_test() {
        let choice = choose_for(&["tools/xtask/src/test/cases/card-reader.rs"]);
        let chosen = names(&choice);
        assert!(!choice.sample);
        assert!(chosen.contains(&"card-reader"));
        assert!(SMOKE.iter().all(|name| chosen.contains(name)));
    }

    #[test]
    fn the_core_runs_the_sample_and_what_covers_it() {
        let choice = choose_for(&["kernel/fs/btrfs/src/tree.rs"]);
        let chosen = names(&choice);
        assert!(choice.sample);
        assert!(SAMPLE.iter().all(|name| chosen.contains(name)), "{chosen:?}");
        assert!(chosen.contains(&"btrfs-stress"), "{chosen:?}");
        assert!(chosen.contains(&"btrfs-raid-pull"), "{chosen:?}");
        // Not every test: the network's beyond the sample stay out.
        assert!(!chosen.contains(&"network-wireguard"), "{chosen:?}");
    }

    #[test]
    fn an_unnamed_file_runs_the_sample() {
        let choice = choose_for(&["userspace/programs/brand-new/src/main.rs"]);
        assert!(choice.sample);
        let chosen = names(&choice);
        assert!(SAMPLE.iter().all(|name| chosen.contains(name)));
    }

    #[test]
    fn plans_share_the_tests_out_by_time() {
        let all: Vec<&Case> = all_cases().collect();
        assert_eq!(parts(&all), MOST_PARTS);
        let one: Vec<&Case> = all_cases().filter(|case| case.name == "spark").collect();
        assert_eq!(parts(&one), 1);
        assert_eq!(plan(&one), "parts=1\nshards=[1]\n");
    }
}
