// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A power cut at any point of an update leaves the old system or the new
//! one, never a mix of them (ADR 0536). The recording stages the test
//! update into slot B while slot A runs, restarts into it, and lets the
//! health check prove it, with the boot disk behind QEMU's *blklogwrites*
//! driver, which logs every write and flush that reaches it. The boot test
//! itself runs only the recording, as `ob test boot` runs every case.
//! *ob test replay* rebuilds the disk as a power cut at each flush of the
//! log, and between two flushes, would have left it, and boots each: the
//! machine must start slot A or slot B and nothing else, with no kernel
//! panic and no Btrfs error. *sysupdate status* names that slot,
//! *sysupdate verify* reads it back against its signed list, and a stage
//! then goes through from slot A, whatever a half-built slot the cut left.

use super::super::*;
use super::update_stage::{UPDATE_DISKS, UPDATES, power_off, restart, run, stage, started};

pub const CASE: Case = Case {
    name: "update-power-replay",
    about: "the boot disk behind a write log while an update stages, restarts into slot B and proves itself; ob test replay boots the disk as a power cut at each point of the log would leave it, and each starts slot A or slot B whole, never a mix (ADR 0536)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: record,
    disks: UPDATE_DISKS,
    second: None,
};

/// Stages the test update, restarts into it and lets it prove itself, as
/// a recording of the write log: a mark after a `sync` before the stage
/// and another after one at the end note which entries are the update's.
pub fn record(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let version = test_update_version("good");
    started(vm, 'a')?;
    run(vm, "sync; echo \"synced: $((6 * 9))\"", &["synced: 54"])?;
    vm.mark_write_log("stage")?;
    stage(vm, "good")?;
    restart(vm)?;
    started(vm, 'b')?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {version}) proved itself: it is now the default, and slot A \
         (Dukovo {VERSION}) the previous version"
    ))?;
    run(vm, "sync; echo \"synced: $((7 * 8))\"", &["synced: 56"])?;
    vm.mark_write_log("end")?;
    power_off(vm)
}

/// What a machine started from the disk of a power cut must do: start
/// slot A or slot B, name it as the running slot, read back against its
/// signed list, and, on slot A, stage the update.
pub fn check_cut(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let version = test_update_version("good");
    let starts =
        ["spark: started slot a, @system.a", "spark: started slot b, @system.b", PANIC_BANNER];
    let slot = vm.expect_first(&starts).map_err(|err| format!("no slot started: {err}"))?;
    if slot == 2 {
        return Err("the kernel panicked".into());
    }
    vm.expect("; tty1 is shown")?;
    if slot == 0 {
        run(vm, "sysupdate status", &[&format!("Running:  slot A, Dukovo {VERSION}")])?;
        run(
            vm,
            &format!("sysupdate verify {UPDATES}/{CURRENT_RELEASE}"),
            &[&format!(
                "sysupdate: slot A (Dukovo {VERSION}) is the release its signed list names"
            )],
        )?;
        // A stage takes away a slot a power cut left half built.
        stage(vm, "good")?;
    } else {
        run(vm, "sysupdate status", &[&format!("Running:  slot B, Dukovo {version}")])?;
        run(
            vm,
            &format!("sysupdate verify {UPDATES}/good"),
            &[&format!(
                "sysupdate: slot B (Dukovo {version}) is the release its signed list names"
            )],
        )?;
    }
    no_storage_errors(&vm.log())
}

/// Fails if the serial log shows a kernel panic or a Btrfs line that
/// reports an error: a checksum that does not match, or a tree that is
/// damaged, as the disk of a cut that took a mix of old and new writes
/// would show.
fn no_storage_errors(log: &str) -> Result<(), String> {
    if log.contains(PANIC_BANNER) {
        return Err("the kernel panicked".into());
    }
    let error = log.lines().find(|line| {
        let line = line.to_ascii_lowercase();
        line.contains("btrfs")
            && ["error", "corrupt", "checksum", "csum"].iter().any(|word| line.contains(word))
    });
    match error {
        Some(line) => Err(format!("Btrfs reported an error: {}", line.trim())),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_without_complaints_passes() {
        assert!(no_storage_errors("btrfs: mounted slot a\nspark: started slot a\n").is_ok());
    }

    #[test]
    fn a_btrfs_error_or_a_panic_fails() {
        let error = no_storage_errors("spark: x\nBtrfs: error: bad tree block\n").unwrap_err();
        assert!(error.contains("bad tree block"), "{error}");
        assert!(no_storage_errors("btrfs: csum failed at 4096\n").is_err());
        let panic = format!("{PANIC_BANNER} ====================\nRadix panic on CPU 0\n");
        assert!(no_storage_errors(&panic).is_err());
    }

    #[test]
    fn an_error_of_something_else_passes() {
        assert!(no_storage_errors("sysupdate: error: nothing to do\n").is_ok());
    }
}
