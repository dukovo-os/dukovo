// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An update's first start, killed again and again, ends on a slot that
//! works (ADR 0536). The test update is staged and the machine restarted,
//! so that GRUB starts slot B as a try. The machine is then reset, as a
//! power cut would, after a random time of 1 to 40 seconds, again and
//! again: 6 times, or as many as the environment variable OB_TEST_KILLS
//! says. The seed of the times is printed, and OB_TEST_SEED sets it, so
//! that a failure can be run again. After the last kill the machine starts
//! unhindered, and must start slot A, once the update has used up its
//! tries, or slot B, if it proved itself between two kills; *sysupdate
//! status* names that slot as the running one, *sysupdate verify* reads it
//! back against its signed list, and the log holds no kernel panic and no
//! Btrfs error.

use std::time::Duration;

use super::super::*;
use super::update_power_replay::{no_storage_errors, running_slot_is_whole, started_either};
use super::update_stage::{UPDATE_DISKS, restart, stage, started};
use crate::writelog::{Random, clock_seed};

pub const CASE: Case = Case {
    name: "update-first-start-kills",
    about: "the machine is reset at random times of an update's first start, again and again, and then starts slot A or slot B whole, which sysupdate status names and sysupdate verify reads back against its signed list (ADR 0536)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: kill_the_first_start,
    disks: UPDATE_DISKS,
    second: None,
};

/// How many times the first start is killed without OB_TEST_KILLS.
const KILLS: u64 = 6;

/// The shortest and the longest time, in seconds, that the machine runs
/// before a kill.
const RUNS: (u64, u64) = (1, 40);

/// The number the environment variable `name` holds, or `default` without
/// it.
fn number_from_environment(name: &str, default: u64) -> Result<u64, String> {
    parse_number(name, std::env::var(name).ok().as_deref(), default)
}

/// `value`, the content of the environment variable `name`, as a number,
/// or `default` if the variable is not set.
fn parse_number(name: &str, value: Option<&str>, default: u64) -> Result<u64, String> {
    match value {
        Some(value) => value
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("{name} takes a number, not `{value}`")),
        None => Ok(default),
    }
}

/// The times, in seconds, that the machine runs before each of `kills`
/// kills, drawn from `seed`.
fn run_times(seed: u64, kills: u64) -> Vec<u64> {
    let mut random = Random::new(seed);
    (0..kills).map(|_| RUNS.0 + random.below(RUNS.1 - RUNS.0 + 1)).collect()
}

/// Stages the update, restarts into it, kills its start again and again,
/// and checks the start that is let through.
pub fn kill_the_first_start(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let kills = number_from_environment("OB_TEST_KILLS", KILLS)?;
    let seed = number_from_environment("OB_TEST_SEED", clock_seed())?;
    eprintln!("    first-start kills: {kills} of them, seed {seed} (OB_TEST_KILLS and OB_TEST_SEED set them)");
    started(vm, 'a')?;
    stage(vm, "good")?;
    restart(vm)?;
    for (kill, seconds) in run_times(seed, kills).into_iter().enumerate() {
        std::thread::sleep(Duration::from_secs(seconds));
        eprintln!("    kill {} of {kills}, after {seconds} s", kill + 1);
        vm.monitor("system_reset")?;
    }
    // Only the start that follows the last kill counts.
    vm.skip_output();
    let slot = started_either(vm)?;
    // The start after a fallback says so on the screen's bottom line until
    // a key is pressed (ADR 0501); Shift takes it off.
    vm.press(&["shift"])?;
    running_slot_is_whole(vm, slot)?;
    no_storage_errors(&vm.log())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_times_come_from_the_seed_within_the_bounds() {
        let times = run_times(7, 50);
        assert_eq!(times.len(), 50);
        assert!(times.iter().all(|seconds| (RUNS.0..=RUNS.1).contains(seconds)), "{times:?}");
        assert_eq!(times, run_times(7, 50));
        assert_ne!(times, run_times(8, 50));
        assert!(times.iter().min() < times.iter().max(), "every time is the same");
    }

    #[test]
    fn no_kills_take_no_time() {
        assert!(run_times(1, 0).is_empty());
    }

    #[test]
    fn the_environment_gives_a_number_or_the_default() {
        assert_eq!(parse_number("OB_TEST_KILLS", Some("20"), 6), Ok(20));
        assert_eq!(parse_number("OB_TEST_KILLS", Some(" 7\n"), 6), Ok(7));
        assert_eq!(parse_number("OB_TEST_KILLS", None, 6), Ok(6));
    }

    #[test]
    fn a_value_that_is_not_a_number_is_refused_by_name() {
        let why = parse_number("OB_TEST_SEED", Some("soon"), 1).unwrap_err();
        assert_eq!(why, "OB_TEST_SEED takes a number, not `soon`");
        assert!(parse_number("OB_TEST_SEED", Some("-3"), 1).is_err());
    }
}
