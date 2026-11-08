// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install log: every step, every command the installer runs with
//! what it printed, and every problem, each line stamped with the seconds
//! since the install started. It is written to `/tmp/install.log` as the
//! install runs, so that the rescue shell on another console can follow
//! it, and copied into the installed system as
//! `/system/state/log/install.log`. When an install fails, read the log
//! from its end.
//!
//! The installer's screens watch the log as the install runs in another
//! thread: [`Log::watched`] sends them each step as it starts, each line
//! as it is written, and how far the copy has got ([`Event`]).

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Instant;

use crate::answers::Layout;
use crate::messages::{self, Arg};

/// Where the log is written while the installer runs.
pub const LIVE_PATH: &str = "/tmp/install.log";

/// A step of the install, in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The release's signatures are checked.
    Release,
    /// The target disk is checked.
    Disk,
    /// Windows' volume is shrunk, to install next to it.
    Shrink,
    /// The disk is partitioned.
    Partition,
    /// Its filesystems are made.
    Format,
    /// The root's Btrfs is made a RAID1 of the mirror layout's two disks.
    Mirror,
    /// The root's subvolumes are made.
    Subvolumes,
    /// The system is copied into its slot, every file checked.
    Copy,
    /// The settings and accounts are written.
    Settings,
    /// GRUB goes on the disk.
    Grub,
    /// The answers and the log are saved in the new system.
    Save,
}

impl Step {
    /// Every step, in the order they run.
    pub const ALL: [Step; 11] = [
        Step::Release,
        Step::Disk,
        Step::Shrink,
        Step::Partition,
        Step::Format,
        Step::Mirror,
        Step::Subvolumes,
        Step::Copy,
        Step::Settings,
        Step::Grub,
        Step::Save,
    ];

    /// The steps an install of `layout` runs, in order: Windows' volume
    /// is shrunk only next to it, and the root mirrored only by the
    /// mirror layout.
    pub fn of(layout: Layout) -> impl Iterator<Item = Step> {
        Step::ALL.into_iter().filter(move |&step| match step {
            Step::Shrink => layout == Layout::Beside,
            Step::Mirror => layout == Layout::Mirror,
            _ => true,
        })
    }

    /// The id of its title among the installer's messages.
    fn message(self) -> &'static str {
        match self {
            Step::Release => "step-release",
            Step::Disk => "step-disk",
            Step::Shrink => "step-shrink",
            Step::Partition => "step-partition",
            Step::Format => "step-format",
            Step::Mirror => "step-mirror",
            Step::Subvolumes => "step-subvolumes",
            Step::Copy => "step-copy",
            Step::Settings => "step-settings",
            Step::Grub => "step-grub",
            Step::Save => "step-save",
        }
    }

    /// Its title, for an install onto `disk` of the release `version`.
    #[must_use]
    pub fn title(self, disk: &str, version: &str) -> String {
        messages::with(
            self.message(),
            &[("disk", Arg::Text(disk)), ("version", Arg::Text(version))],
        )
    }

    /// About how much of an install's time it takes, out of 100, for the
    /// screens' progress bar: the copy takes most of it.
    #[must_use]
    pub fn share(self) -> u32 {
        match self {
            Step::Copy => 70,
            Step::Format | Step::Grub | Step::Shrink => 8,
            Step::Release => 6,
            Step::Partition | Step::Mirror | Step::Subvolumes | Step::Settings => 2,
            Step::Disk | Step::Save => 1,
        }
    }
}

/// What a watched log tells its watcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A step started.
    Step(Step),
    /// A line was written to the log.
    Line(String),
    /// The copy has written `done` of the release's `total` bytes.
    Copied {
        /// Bytes written so far.
        done: u64,
        /// The release's bytes.
        total: u64,
    },
}

/// The install log.
pub struct Log {
    file: File,
    path: PathBuf,
    started: Instant,
    /// Whether steps are shown on the console too.
    shown: bool,
    /// Who is told of each step and line, if anyone: the screens.
    watcher: Option<Sender<Event>>,
    /// The disk being installed onto and the release's version, which
    /// the steps' titles name.
    disk: String,
    version: String,
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
        let mut log = Log {
            file,
            path: path.to_path_buf(),
            started: Instant::now(),
            shown,
            watcher: None,
            disk: String::new(),
            version: String::new(),
        };
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        log.write(&format!(
            "dukovo-install {}, started at {} seconds since 1970",
            env!("CARGO_PKG_VERSION"),
            now.as_secs()
        ));
        Ok(log)
    }

    /// The log, telling `watcher` of each step and line from now on.
    #[must_use]
    pub fn watched(mut self, watcher: Sender<Event>) -> Log {
        self.watcher = Some(watcher);
        self
    }

    /// Where the log is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Notes that the install is onto `disk`, of the release `version`,
    /// which the steps' titles name from then on.
    pub fn installing(&mut self, disk: &str, version: &str) {
        disk.clone_into(&mut self.disk);
        version.clone_into(&mut self.version);
    }

    /// Writes `text` to the log, a line at a time, each stamped. A log
    /// that can no longer be written must not stop the install, so a
    /// failed write is left out.
    fn write(&mut self, text: &str) {
        let seconds = self.started.elapsed().as_secs_f32();
        for line in text.lines() {
            let _ = writeln!(self.file, "[{seconds:7.1}] {line}");
            self.tell(Event::Line(line.to_string()));
        }
    }

    /// Tells the watcher of `event`. A watcher that has gone, as when the
    /// screens ended, does not stop the install.
    fn tell(&self, event: Event) {
        if let Some(watcher) = &self.watcher {
            let _ = watcher.send(event);
        }
    }

    /// Starts a step: in the log, and on the console.
    pub fn step(&mut self, step: Step) {
        let title = step.title(&self.disk, &self.version);
        self.tell(Event::Step(step));
        self.write(&format!("==> {title}"));
        if self.shown {
            println!("dukovo-install: {title}");
        }
    }

    /// Notes that the copy has written `done` of `total` bytes, for the
    /// watcher alone.
    pub fn copied(&self, done: u64, total: u64) {
        self.tell(Event::Copied { done, total });
    }

    /// Shows `text` on the console as the installer's last word, and
    /// notes it in the log.
    pub fn finish(&mut self, text: &str) {
        self.write(&format!("==> {text}"));
        if self.shown {
            println!("dukovo-install: {text}");
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
