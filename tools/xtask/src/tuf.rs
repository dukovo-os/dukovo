// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob test tuf`: runs the TUF conformance suite against Dukovo's TUF
//! client (ADR 0380), through its conformance CLI built for the host.
//!
//! The suite comes from the commit of tuf-conformance pinned here, which
//! `ob` fetches once into `.cache/tuf-conformance/`, and runs in a Python
//! virtual environment there with the versions of python-tuf,
//! securesystemslib and pytest its pyproject.toml pins. It needs Python
//! 3.10 or later with venv, and faketime, which some tests run the client
//! under. The suite is MIT-licensed, its dependencies MIT or Apache-2.0,
//! and all of it only runs on the build host.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result};

/// The tuf-conformance commit the client is tested against.
pub const SUITE_COMMIT: &str = "8535468cf76f5395a579809c4e72ca5698a2a1cd";

/// Where it comes from.
const SUITE_REPOSITORY: &str = "https://github.com/theupdateframework/tuf-conformance.git";

/// What its pyproject.toml pins, less the JSON report it writes for CI.
const REQUIREMENTS: &[&str] = &["securesystemslib[crypto]==1.6.0", "tuf==7.0.1", "pytest==9.2.0"];

/// The tests the client is expected to fail, each with why, which the
/// suite reads from beside the client as `<client>.xfails`.
pub const XFAILS: &str = "userspace/programs/tuf/tuf-conformance.xfails";

/// How to install what the suite needs.
pub const INSTALL: &str = "install Python 3.10 or later with venv, and faketime (Debian, Ubuntu, \
                           Mint: `sudo apt install python3-venv faketime`)";

/// Builds the client and runs the suite against it.
pub fn check(ctx: &Ctx) -> Result<()> {
    for program in ["python3", "faketime", "git"] {
        if process::find_program(program).is_none() {
            return Err(
                Error::new(format!("{program} is not installed; the suite needs it")).hint(INSTALL)
            );
        }
    }
    let suite = suite(ctx)?;
    let python = venv(ctx)?;
    step("Building the TUF client's conformance CLI for the host");
    let mut cmd = crate::userspace::host_cargo_of(ctx, "build", &["tuf"]);
    cmd.args(["--bin", "tuf-conformance"]);
    process::run(ctx, &mut cmd)?;
    let client = ctx.target.join("programs-host/debug/tuf-conformance");
    let xfails = client.with_extension("xfails");
    fs::copy(ctx.root.join(XFAILS), &xfails)
        .context(|| format!("copying {XFAILS} to {}", xfails.display()))?;
    step(&format!("Running the TUF conformance suite at {}", &SUITE_COMMIT[..12]));
    let mut cmd = Command::new(python);
    cmd.current_dir(&suite)
        .args(["-m", "pytest", "-q", "-p", "no:cacheprovider", "tuf_conformance", "--entrypoint"])
        .arg(&client);
    process::run(ctx, &mut cmd)
}

/// The suite at the pinned commit, fetched if it is not there yet.
fn suite(ctx: &Ctx) -> Result<PathBuf> {
    let dir = ctx.root.join(".cache/tuf-conformance").join(SUITE_COMMIT);
    if dir.join("tuf_conformance/conftest.py").is_file() {
        return Ok(dir);
    }
    step(&format!("Fetching tuf-conformance {}", &SUITE_COMMIT[..12]));
    let partial = dir.with_extension("partial");
    if partial.exists() {
        fs::remove_dir_all(&partial).context(|| format!("removing {}", partial.display()))?;
    }
    process::create_dir(&partial)?;
    let git = |args: &[&str]| {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&partial).args(args);
        process::run(ctx, &mut cmd)
    };
    git(&["init", "-q"])?;
    git(&["fetch", "-q", "--depth", "1", SUITE_REPOSITORY, SUITE_COMMIT])?;
    git(&["checkout", "-q", "FETCH_HEAD"])?;
    let head = process::output(
        ctx,
        Command::new("git").arg("-C").arg(&partial).args(["rev-parse", "HEAD"]),
    )?;
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    if head != SUITE_COMMIT {
        return Err(Error::new(format!("fetched tuf-conformance {head}, not {SUITE_COMMIT}")));
    }
    fs::rename(&partial, &dir).context(|| format!("moving it to {}", dir.display()))?;
    Ok(dir)
}

/// The Python of a virtual environment with the suite's requirements,
/// made if it is not there yet.
fn venv(ctx: &Ctx) -> Result<PathBuf> {
    // A change to the requirements makes another environment.
    let key: u32 =
        REQUIREMENTS.join(" ").bytes().fold(5381, |h, b| h.wrapping_mul(33) ^ u32::from(b));
    let dir = ctx.root.join(".cache/tuf-conformance").join(format!("venv-{key:08x}"));
    let python = dir.join("bin/python");
    let done = dir.join(".installed");
    if done.is_file() {
        return Ok(python);
    }
    step("Installing the suite's Python requirements");
    if dir.exists() {
        fs::remove_dir_all(&dir).context(|| format!("removing {}", dir.display()))?;
    }
    let mut cmd = Command::new("python3");
    cmd.args(["-m", "venv"]).arg(&dir);
    process::run(ctx, &mut cmd).map_err(|err| err.hint(INSTALL))?;
    let mut cmd = Command::new(&python);
    cmd.args(["-m", "pip", "install", "--quiet", "--disable-pip-version-check"]).args(REQUIREMENTS);
    process::run(ctx, &mut cmd)?;
    fs::write(&done, REQUIREMENTS.join("\n")).context(|| format!("writing {}", done.display()))?;
    Ok(python)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_expected_failure_says_why() {
        let text = include_str!("../../../userspace/programs/tuf/tuf-conformance.xfails");
        let mut why = false;
        for line in text.lines() {
            if line.starts_with('#') {
                why = !line.starts_with("# SPDX");
            } else if !line.trim().is_empty() {
                assert!(why, "{XFAILS}: {line} has no comment above it saying why");
            } else {
                why = false;
            }
        }
    }

    #[test]
    fn ci_runs_the_suite() {
        let ci = include_str!("../../../.github/workflows/ci.yml");
        assert!(
            ci.contains("./ob test tuf"),
            ".github/workflows/ci.yml does not run `ob test tuf`"
        );
    }
}
