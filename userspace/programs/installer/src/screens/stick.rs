// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Setup files on the install stick. A setup file is an answers file
//! called `install.json` at the top of the stick's EFI system partition:
//! FAT, which any computer can write, so that one can be put there
//! before the stick is plugged in, and the Done page saves one there for
//! the next install. Spark leaves EFI system partitions unmounted, so the
//! partition is mounted for each read or write and unmounted after.

use std::fs;
use std::path::Path;
use std::process::Command;

use super::app::SetupFile;
use crate::answers::Answers;
use crate::messages::{self, Arg};

/// A setup file's name.
pub const SETUP_FILE: &str = "install.json";

/// The GPT type of an EFI system partition.
const EFI_SYSTEM: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

/// Where the stick's EFI system partition is mounted.
const MOUNTED: &str = "/tmp/install/stick";

/// The EFI system partition of the disk `stick`, by its name.
fn esp(stick: &str) -> Option<String> {
    let entries = fs::read_dir("/kernel/block").ok()?;
    let mut names: Vec<String> =
        entries.flatten().filter_map(|entry| entry.file_name().into_string().ok()).collect();
    names.sort();
    names.into_iter().find(|name| {
        crate::disks::disk_of(name) == stick
            && name != stick
            && fs::read_to_string(Path::new("/kernel/block").join(name).join("parttype"))
                .is_ok_and(|kind| kind.trim().eq_ignore_ascii_case(EFI_SYSTEM))
    })
}

/// Runs `/system/commands/<program>` with `args`.
fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(Path::new("/system/commands").join(program))
        .args(args)
        .output()
        .map_err(|err| format!("{program}: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Mounts the stick's EFI system partition, read-only unless `write`,
/// runs `work` in it, and unmounts it.
fn with_esp<T>(stick: &str, write: bool, work: impl FnOnce(&Path) -> T) -> Result<T, String> {
    let part = esp(stick).ok_or_else(|| messages::text("setup-no-esp"))?;
    fs::create_dir_all(MOUNTED).map_err(|err| format!("{MOUNTED}: {err}"))?;
    let device = format!("/devices/{part}");
    run("mount", &["-t", "fat", "-o", if write { "rw" } else { "ro" }, &device, MOUNTED])?;
    let done = work(Path::new(MOUNTED));
    run("umount", &[MOUNTED])?;
    Ok(done)
}

/// The setup files on the stick `stick`, each with its answers or what
/// is wrong with them.
#[must_use]
pub fn setup_files(stick: Option<&str>) -> Vec<SetupFile> {
    let Some(stick) = stick else { return Vec::new() };
    let read = with_esp(stick, false, |dir| fs::read(dir.join(SETUP_FILE)).ok());
    match read {
        Ok(Some(bytes)) => {
            let answers = Answers::parse(&bytes).map_err(|why| vec![why]).and_then(|answers| {
                answers.check()?;
                Ok(answers)
            });
            let place = messages::with("setup-on-stick", &[("file", Arg::Text(SETUP_FILE))]);
            vec![SetupFile { place, answers }]
        }
        Ok(None) | Err(_) => Vec::new(),
    }
}

/// Saves `answers` as the setup file on the stick `stick`, and says what
/// was done.
#[must_use]
pub fn save(stick: Option<&str>, answers: &Answers) -> String {
    let Some(stick) = stick else { return messages::text("setup-no-esp") };
    let saved = with_esp(stick, true, |dir| fs::write(dir.join(SETUP_FILE), answers.to_saved()));
    match saved {
        Ok(Ok(())) if answers.keep_password_hashes => messages::text("setup-saved"),
        Ok(Ok(())) => messages::text("setup-saved-no-hashes"),
        Ok(Err(err)) => format!("{SETUP_FILE}: {err}"),
        Err(why) => why,
    }
}
