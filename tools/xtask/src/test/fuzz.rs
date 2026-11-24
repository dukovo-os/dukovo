// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob test fuzz`: runs the cargo-fuzz targets of the update path's
//! parsers and verifiers (ADR 0536), and linuxd's netlink request parser
//! (ADR 0621), which are in `userspace/programs/fuzz`.
//!
//! Each target runs for `--seconds` (60 without it) on the corpus kept in
//! `.cache/fuzz/<target>/`, which starts from the seeds in git under
//! `userspace/programs/fuzz/corpus/<target>/` and grows with each run. A
//! crash fails the run and leaves its input in `out/fuzz/<target>/`;
//! `cargo fuzz run` with that file as its argument runs the crash again.
//! The fuzz crate builds with cargo-fuzz's sanitizers into
//! `target/fuzz`, apart from every other build.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::doctor;
use crate::process::{self, step};
use crate::{Args, Context, Ctx, Error, Result};

/// The fuzz targets, each named for the file in the fuzz crate's
/// `fuzz_targets` folder.
pub const TARGETS: [&str; 10] = [
    "release-list",
    "tuf-root",
    "tuf-targets",
    "tuf-snapshot",
    "tuf-timestamp",
    "rollout",
    "advisories",
    "canary-report",
    "slot-state",
    "linuxd-netlink",
];

/// The fuzz crate, relative to the root of the tree.
const FUZZ_DIR: &str = "userspace/programs/fuzz";

/// How long a target runs without `--seconds`.
const DEFAULT_SECONDS: u64 = 60;

/// The names libFuzzer gives the input of a run that failed.
const CRASH_PREFIXES: [&str; 4] = ["crash-", "oom-", "timeout-", "leak-"];

/// What `ob test fuzz` takes.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Options {
    target: Option<String>,
    seconds: Option<u64>,
}

impl Options {
    /// Takes `--target` and `--seconds` from `args`.
    pub(super) fn take(args: &mut Args) -> Result<Options> {
        let target = args.value("--target")?;
        if let Some(name) = &target
            && !TARGETS.contains(&name.as_str())
        {
            crate::bail!("`{name}` is no fuzz target; the targets are {}", TARGETS.join(", "));
        }
        let seconds = match args.value("--seconds")? {
            Some(value) => {
                let seconds =
                    value.parse::<u64>().ok().filter(|&secs| secs > 0).ok_or_else(|| {
                        Error::new(format!("--seconds takes a number of seconds, not `{value}`"))
                    })?;
                Some(seconds)
            }
            None => None,
        };
        Ok(Options { target, seconds })
    }

    /// Fails if an option was given, for a command that is not `fuzz`.
    pub(super) fn none_given(&self) -> Result<()> {
        if *self != Options::default() {
            crate::bail!("--target and --seconds belong to `ob test fuzz`; give `fuzz` with them");
        }
        Ok(())
    }

    /// The targets to run: the one named, or all.
    fn targets(&self) -> Vec<&str> {
        match &self.target {
            Some(name) => TARGETS.iter().copied().filter(|target| target == name).collect(),
            None => TARGETS.to_vec(),
        }
    }
}

/// Runs the targets `options` selects, and fails if any found a crash.
pub(super) fn command(ctx: &Ctx, options: &Options) -> Result<()> {
    match doctor::cargo_fuzz_version().as_deref() {
        Some(version) if version == doctor::CARGO_FUZZ_VERSION => {}
        Some(version) => {
            return Err(Error::new(format!(
                "cargo-fuzz {version} is installed, but the targets run with {}",
                doctor::CARGO_FUZZ_VERSION
            ))
            .hint(doctor::cargo_fuzz_install()));
        }
        None => {
            return Err(
                Error::new("cargo-fuzz is not installed").hint(doctor::cargo_fuzz_install())
            );
        }
    }
    let seconds = options.seconds.unwrap_or(DEFAULT_SECONDS);
    let mut crashed = Vec::new();
    for target in options.targets() {
        step(&format!("Fuzzing {target} for {seconds} seconds"));
        if let Some(input) = fuzz(ctx, target, seconds)? {
            crashed.push((target, input));
        }
    }
    if crashed.is_empty() {
        return Ok(());
    }
    let mut message = String::from("a fuzz target found an input that makes it fail:");
    for (target, input) in &crashed {
        message.push_str(&format!("\n  {target}: {}", input.display()));
    }
    Err(Error::new(message).hint(format!(
        "run it again with `cargo fuzz run --fuzz-dir {FUZZ_DIR} <target> <input>`, and add the \
         input to {FUZZ_DIR}/corpus/<target>/ once the fault is fixed"
    )))
}

/// Runs `target` for `seconds` on its corpus. Gives the input a crash
/// left in `out/fuzz/<target>/`, if the run found one.
fn fuzz(ctx: &Ctx, target: &str, seconds: u64) -> Result<Option<PathBuf>> {
    let corpus = ctx.root.join(".cache/fuzz").join(target);
    let out = ctx.root.join("out/fuzz").join(target);
    seed(&ctx.root.join(FUZZ_DIR).join("corpus").join(target), &corpus)?;
    forget_crashes(&out)?;
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    // The ob wrapper (ADR 0535) is left out: cargo-fuzz sets the
    // compiler's flags itself, and the targets are never an artefact of a
    // release.
    cmd.current_dir(&ctx.root).env("CARGO_TARGET_DIR", ctx.target.join("fuzz"));
    for var in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET"] {
        cmd.env_remove(var);
    }
    cmd.args(["fuzz", "run", "--fuzz-dir", FUZZ_DIR, target]).arg(&corpus).arg("--");
    // libFuzzer takes the folder of the crash's input as a prefix, so it
    // ends in a slash.
    cmd.arg(format!("-max_total_time={seconds}"))
        .arg(format!("-artifact_prefix={}/", out.display()));
    let status = process::status(ctx, &mut cmd)?;
    if status.success() {
        return Ok(None);
    }
    match crashes(&out)?.into_iter().next() {
        Some(input) => Ok(Some(input)),
        None => Err(Error::new(format!("fuzzing {target} failed ({status}) with no crash input"))
            .hint(
                "the output above shows why: a build that fails, or a fuzzer that does not start",
            )),
    }
}

/// Copies the seeds of a target that the corpus lacks into the corpus.
fn seed(seeds: &Path, corpus: &Path) -> Result<()> {
    process::create_dir(corpus)?;
    let entries = std::fs::read_dir(seeds).context(|| format!("reading {}", seeds.display()))?;
    for entry in entries {
        let entry = entry.context(|| format!("reading {}", seeds.display()))?;
        let to = corpus.join(entry.file_name());
        if !to.exists() {
            std::fs::copy(entry.path(), &to)
                .context(|| format!("copying {} to {}", entry.path().display(), to.display()))?;
        }
    }
    Ok(())
}

/// Takes the inputs of earlier crashes out of `out`, so that the ones it
/// holds after a run are that run's.
fn forget_crashes(out: &Path) -> Result<()> {
    process::create_dir(out)?;
    for path in crashes(out)? {
        std::fs::remove_file(&path).context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

/// The inputs libFuzzer left in `out`, by name.
fn crashes(out: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let entries = std::fs::read_dir(out).context(|| format!("reading {}", out.display()))?;
    for entry in entries {
        let path = entry.context(|| format!("reading {}", out.display()))?.path();
        if path.file_name().and_then(OsStr::to_str).is_some_and(is_crash) {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// Whether `name` is one libFuzzer gives a failing input.
fn is_crash(name: &str) -> bool {
    CRASH_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(args: &[&str]) -> Result<Options> {
        let mut args = Args::new("test", args.iter().map(ToString::to_string).collect());
        Options::take(&mut args)
    }

    #[test]
    fn every_target_has_a_file_and_a_seed() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(FUZZ_DIR);
        for target in TARGETS {
            assert!(root.join("fuzz_targets").join(format!("{target}.rs")).is_file(), "{target}");
            let seeds = std::fs::read_dir(root.join("corpus").join(target))
                .unwrap_or_else(|_| panic!("{target} has no seed corpus"));
            assert!(seeds.count() > 0, "{target} has no seed");
        }
    }

    #[test]
    fn the_fuzz_crate_lists_each_target_as_a_binary() {
        let manifest = include_str!("../../../../userspace/programs/fuzz/Cargo.toml");
        for target in TARGETS {
            let entry = format!("name = \"{target}\"\npath = \"fuzz_targets/{target}.rs\"");
            assert!(manifest.contains(&entry), "{target}");
        }
    }

    #[test]
    fn a_target_and_a_time_are_taken_and_others_refused() {
        let taken = options(&["--target", "slot-state", "--seconds", "30"]).unwrap();
        assert_eq!(taken.targets(), ["slot-state"]);
        assert_eq!(taken.seconds, Some(30));
        assert_eq!(options(&[]).unwrap().targets(), TARGETS);
        assert!(options(&["--target", "kernel"]).is_err());
        assert!(options(&["--seconds", "0"]).is_err());
        assert!(options(&["--seconds", "soon"]).is_err());
    }

    #[test]
    fn options_given_to_another_suite_are_refused() {
        assert!(options(&["--seconds", "5"]).unwrap().none_given().is_err());
        assert!(options(&[]).unwrap().none_given().is_ok());
    }

    #[test]
    fn libfuzzers_failing_inputs_are_told_from_the_rest() {
        for name in ["crash-0a1b", "oom-2c", "timeout-3d", "leak-4e"] {
            assert!(is_crash(name), "{name}");
        }
        assert!(!is_crash("0a1b2c"));
    }
}
