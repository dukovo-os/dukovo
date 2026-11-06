// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install log: every step, every command the installer runs with
//! what it printed, and every problem, each line stamped with the seconds
//! since the install started. It is written to `/tmp/install.log` as the
//! install runs, so that the rescue shell on another console can follow
//! it, and copied into the installed system as
//! `/system/state/log/install.log`. When an install fails, read the log
//! from its end.

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// Where the log is written while the installer runs.
pub const LIVE_PATH: &str = "/tmp/install.log";

/// The install log.
pub struct Log {
    file: File,
    path: PathBuf,
    started: Instant,
    /// Whether steps are shown on the console too.
    shown: bool,
}

impl Log {
    /// Makes the log at `path`, with a first line naming the installer
    /// and when it started. With `shown`, steps and problems are printed
    /// too.
    ///
    /// # Errors
    ///
    /// If the file cannot be made.
    pub fn create(path: &Path, shown: bool) -> Result<Log, String> {
        let file = File::create(path).map_err(|err| format!("making {}: {err}", path.display()))?;
        let mut log = Log { file, path: path.to_path_buf(), started: Instant::now(), shown };
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        log.write(&format!(
            "dukovo-install {}, started at {} seconds since 1970",
            env!("CARGO_PKG_VERSION"),
            now.as_secs()
        ));
        Ok(log)
    }

    /// Where the log is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes `text` to the log, a line at a time, each stamped. A log
    /// that can no longer be written must not stop the install, so a
    /// failed write is left out.
    fn write(&mut self, text: &str) {
        let seconds = self.started.elapsed().as_secs_f32();
        for line in text.lines() {
            let _ = writeln!(self.file, "[{seconds:7.1}] {line}");
        }
    }

    /// Starts a step: in the log, and on the console.
    pub fn step(&mut self, title: &str) {
        self.write(&format!("==> {title}"));
        if self.shown {
            println!("dukovo-install: {title}");
        }
    }

    /// Notes `text` in the log only.
    pub fn note(&mut self, text: &str) {
        self.write(text);
    }

    /// Notes a problem: in the log, and on the console.
    pub fn problem(&mut self, text: &str) {
        self.write(&format!("problem: {text}"));
        if self.shown {
            eprintln!("dukovo-install: {text}");
        }
    }

    /// Runs `/system/commands/<program>` with `args`, noting its command
    /// line and what it printed, and gives what it printed on its
    /// standard output.
    ///
    /// # Errors
    ///
    /// If it cannot be started or fails, with the last lines it printed.
    pub fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
        self.write(&format!("$ {program} {}", args.join(" ")));
        let output = Command::new(Path::new("/system/commands").join(program))
            .args(args)
            .output()
            .map_err(|err| format!("starting {program}: {err}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr);
        self.write(&stdout);
        self.write(&stderr);
        if output.status.success() {
            return Ok(stdout);
        }
        let said: Vec<&str> =
            stderr.lines().chain(stdout.lines()).filter(|l| !l.is_empty()).collect();
        let last = said.iter().rev().take(3).rev().copied().collect::<Vec<_>>().join(" / ");
        Err(format!("{program} {} failed ({}): {last}", args.join(" "), output.status))
    }
}
