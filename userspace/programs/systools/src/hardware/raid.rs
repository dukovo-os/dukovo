// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The RAID area of `hardware check`: for each controller in
//! `/kernel/raid` (ADR 0664), whether it is in fault, whether each of its
//! volumes is optimal, whether any disk behind it has failed or is
//! predicted to fail, and whether its battery holds.
//!
//! The controller's own answers are read, never the disks: the driver
//! keeps them from the controller's last answers, so this area takes no
//! time. A disk that is fine has no line of its own, only the count in the
//! line of the controller's disks; one that is not has a line, named by
//! its bay. The disks' own health (their SMART or log pages, read
//! through the controller) is in the disks area, which lists them as
//! `raid0:bay4`.

use std::path::Path;

use super::check::{Area, Check, Problem, counted};
use crate::raid::{self, Battery, Controller, Disk, Volume};
use crate::util;

/// What a volume is called in a line: "raid0 volume 0".
fn volume_name(controller: &Controller, volume: &Volume) -> String {
    format!("raid{} volume {}", controller.n, volume.target)
}

/// The words after a volume's level and size that say what is running on
/// it, as "rebuilding Bay 3, 42%", or nothing.
fn rebuilding(controller: &Controller, volume: &Volume) -> Option<String> {
    let disk = controller.disks.iter().find(|disk| {
        disk.volume == Some(volume.target) && disk.task.as_deref() == Some("rebuild")
    })?;
    let place = disk.bay.clone().unwrap_or_else(|| format!("disk {}", disk.id));
    let percent = disk.percent.map_or_else(String::new, |percent| format!(", {percent}%"));
    Some(format!("rebuilding {place}{percent}"))
}

/// The check of one volume.
fn volume_check(controller: &Controller, volume: &Volume) -> Check {
    let name = volume_name(controller, volume);
    let level = volume.level_words();
    let size =
        volume.size.map_or_else(String::new, |size| format!(" {}", util::human(size, false)));
    let state = volume.state.as_deref().unwrap_or("not known");
    let mut detail = format!("{level}{size}: {state}");
    if volume.is_optimal() {
        return Check::passed(Area::Raid, &name, detail);
    }
    detail.push_str(&rebuilding(controller, volume).map_or_else(String::new, |r| format!(", {r}")));
    match volume.state.as_deref() {
        Some("partly-degraded") => Check::warning(Area::Raid, &name, Problem::RaidVolume, detail),
        _ => Check::failed(Area::Raid, &name, Problem::RaidVolume, detail),
    }
}

/// What a disk is called in a line: "raid0 Bay 3".
fn disk_name(controller: &Controller, disk: &Disk) -> String {
    let place = disk.bay.clone().unwrap_or_else(|| format!("disk {}", disk.id));
    format!("raid{} {place}", controller.n)
}

/// The model and serial number of a disk, to find it by in a line.
fn disk_words(disk: &Disk) -> String {
    let parts: Vec<&str> =
        [disk.vendor.as_deref(), disk.model.as_deref()].into_iter().flatten().collect();
    let model = if parts.is_empty() { "the disk".to_string() } else { parts.join(" ") };
    match &disk.serial {
        Some(serial) => format!("{model}, serial {serial}"),
        None => model,
    }
}

/// The checks of the disks: a line for each that has failed or is
/// predicted to, and one of the rest.
fn disk_checks(controller: &Controller) -> Vec<Check> {
    let mut checks = Vec::new();
    let mut fine = 0u64;
    for disk in &controller.disks {
        let name = disk_name(controller, disk);
        if disk.has_failed() {
            let detail = format!("{} has failed", disk_words(disk));
            checks.push(Check::failed(Area::Raid, &name, Problem::RaidDiskFailed, detail));
        } else if disk.is_predicted_to_fail() {
            let detail = format!("{} is predicted to fail", disk_words(disk));
            checks.push(Check::failed(Area::Raid, &name, Problem::RaidDiskPredicted, detail));
        } else {
            fine += 1;
        }
    }
    let name = format!("raid{} disks", controller.n);
    let detail = format!("{} fine; none failed or predicted to fail", counted(fine, "disk"));
    if checks.is_empty() {
        checks.push(Check::passed(Area::Raid, &name, detail));
    } else if fine > 0 {
        checks.push(Check::passed(
            Area::Raid,
            &name,
            format!("{} fine", counted(fine, "other disk")),
        ));
    }
    checks
}

/// The check of the battery.
fn battery_check(controller: &Controller, battery: &Battery) -> Check {
    let name = format!("raid{} battery", controller.n);
    let state = battery.state.as_deref().unwrap_or("not known");
    let mut detail = state.to_string();
    if let Some(charge) = battery.charge {
        detail.push_str(&format!(", {charge}% charged"));
    }
    if battery.has_failed() {
        return Check::failed(Area::Raid, &name, Problem::RaidBattery, detail);
    }
    if let Some(learn @ ("failed" | "timed-out")) = battery.learn.as_deref() {
        detail.push_str(&format!(", its last test of its capacity {learn}"));
    }
    if battery.is_worn() {
        return Check::warning(Area::Raid, &name, Problem::RaidBattery, detail);
    }
    Check::passed(Area::Raid, &name, detail)
}

/// The check of the controller itself.
fn controller_check(controller: &Controller) -> Check {
    let name = format!("raid{} controller", controller.n);
    let info = &controller.controller;
    let model = info.model.as_deref().unwrap_or("the controller");
    if controller.is_in_fault() {
        let code = info.fault.as_ref().map_or_else(String::new, |code| format!(" (code {code})"));
        let detail = format!("{model} reports a fault{code}");
        return Check::failed(Area::Raid, &name, Problem::RaidController, detail);
    }
    Check::passed(Area::Raid, &name, format!("{model}: working normally"))
}

/// The checks of one controller.
fn checks(controller: &Controller) -> Vec<Check> {
    let mut checks = vec![controller_check(controller)];
    checks.extend(controller.volumes.iter().map(|volume| volume_check(controller, volume)));
    checks.extend(disk_checks(controller));
    match &controller.battery {
        Some(battery) => checks.push(battery_check(controller, battery)),
        None => checks.push(Check::skipped(
            Area::Raid,
            &format!("raid{} battery", controller.n),
            "the controller has no battery".into(),
        )),
    }
    checks
}

/// The checks of the area: those of each controller in `root`, or the one
/// line that says there is none.
pub(crate) fn run(root: &Path) -> Vec<Check> {
    let found = raid::read(root);
    if found.is_empty() {
        return vec![Check::skipped(
            Area::Raid,
            "raid",
            "this machine has no RAID controller".into(),
        )];
    }
    found.iter().flat_map(checks).collect()
}

#[cfg(test)]
mod tests {
    use super::super::check::Outcome;
    use super::*;
    use crate::raid::fixture::Tree;

    fn outcomes(checks: &[Check]) -> Vec<(&str, Outcome, Option<&str>)> {
        checks.iter().map(|c| (c.name.as_str(), c.outcome, c.id.as_deref())).collect()
    }

    #[test]
    fn a_machine_without_a_controller_skips_the_area_with_one_line() {
        let tree = Tree::new();
        let checks = run(tree.path());
        assert_eq!(outcomes(&checks), [("raid", Outcome::Skipped, None)]);
        assert_eq!(checks[0].detail, "this machine has no RAID controller");
    }

    #[test]
    fn a_controller_with_an_optimal_volume_and_sound_disks_passes_every_line() {
        let tree = Tree::new();
        tree.h740p();
        // The disk that rebuilds is of the volume: its rebuild is not a fault in itself.
        let checks = run(tree.path());
        let names: Vec<_> = checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["raid0 controller", "raid0 volume 0", "raid0 disks", "raid0 battery"]);
        assert!(checks.iter().all(|c| c.outcome == Outcome::Passed), "{checks:?}");
        assert_eq!(checks[0].detail, "PERC H740P Mini: working normally");
        assert_eq!(checks[1].detail, "RAID 1 559G: optimal");
        assert_eq!(checks[2].detail, "4 disks fine; none failed or predicted to fail");
        assert_eq!(checks[3].detail, "optimal, 100% charged");
    }

    #[test]
    fn a_degraded_volume_fails_and_says_what_is_rebuilding() {
        let tree = Tree::new();
        tree.h740p();
        tree.put(
            0,
            "volumes",
            "target=0\tlevel=raid1\tsize=599550590976\tstate=degraded\tdisks=0,3\n",
        );
        let checks = run(tree.path());
        assert_eq!((checks[1].outcome, checks[1].id.as_deref()), (Outcome::Failed, Some("HW-171")));
        assert_eq!(checks[1].detail, "RAID 1 559G: degraded, rebuilding Bay 3, 42%");
        tree.put(0, "volumes", "target=0\tlevel=raid10\tstate=partly-degraded\n");
        assert_eq!(run(tree.path())[1].outcome, Outcome::Warning);
        tree.put(0, "volumes", "target=7\tlevel=raid5\tstate=offline\n");
        assert_eq!(run(tree.path())[1].outcome, Outcome::Failed);
        assert_eq!(run(tree.path())[1].detail, "RAID 5: offline");
    }

    #[test]
    fn a_failed_disk_and_a_disk_predicted_to_fail_have_a_line_each() {
        let tree = Tree::new();
        tree.put(
            0,
            "disks",
            "id=0\tenclosure=32\tindex=0\tslot=0\tstate=failed\tvendor=SEAGATE\t\
             model=ST600MM0009\tserial=S0M1\n\
             id=1\tenclosure=32\tindex=0\tslot=1\tstate=online\tsmart-alert=yes\n\
             id=2\tenclosure=32\tindex=0\tslot=2\tstate=online\tpredictive=3\tserial=Q\n\
             id=3\tenclosure=32\tindex=0\tslot=3\tstate=online\n\
             id=4\tstate=unconfigured-bad\n",
        );
        let checks = run(tree.path());
        assert_eq!(
            outcomes(&checks[1..6]),
            [
                ("raid0 Bay 0", Outcome::Failed, Some("HW-172")),
                ("raid0 Bay 1", Outcome::Failed, Some("HW-173")),
                ("raid0 Bay 2", Outcome::Failed, Some("HW-173")),
                ("raid0 disk 4", Outcome::Failed, Some("HW-172")),
                ("raid0 disks", Outcome::Passed, None),
            ]
        );
        assert_eq!(checks[1].detail, "SEAGATE ST600MM0009, serial S0M1 has failed");
        assert_eq!(checks[3].detail, "the disk, serial Q is predicted to fail");
        assert_eq!(checks[5].detail, "1 other disk fine");
    }

    #[test]
    fn a_battery_that_failed_fails_and_one_that_is_worn_warns() {
        let tree = Tree::new();
        tree.put(0, "battery", "state failed\ncharge 3\n");
        let failed = run(tree.path()).pop().unwrap();
        assert_eq!((failed.outcome, failed.id.as_deref()), (Outcome::Failed, Some("HW-174")));
        assert_eq!(failed.detail, "failed, 3% charged");
        tree.put(0, "battery", "state optimal\nlearn timed-out\n");
        let worn = run(tree.path()).pop().unwrap();
        assert_eq!((worn.outcome, worn.id.as_deref()), (Outcome::Warning, Some("HW-174")));
        assert_eq!(worn.detail, "optimal, its last test of its capacity timed-out");
        tree.put(0, "battery", "state charging\ncharge 40\nlearn none\n");
        assert_eq!(run(tree.path()).pop().unwrap().outcome, Outcome::Passed);
    }

    #[test]
    fn a_controller_without_a_battery_skips_that_line() {
        let tree = Tree::new();
        tree.put(0, "controller", "model X\nbattery none\n");
        let last = run(tree.path()).pop().unwrap();
        assert_eq!(last.outcome, Outcome::Skipped);
        assert_eq!(last.detail, "the controller has no battery");
    }

    #[test]
    fn a_controller_in_fault_fails() {
        let tree = Tree::new();
        tree.put(0, "controller", "model PERC H740P Mini\nstate fault\nfault 0x00002000\n");
        let first = run(tree.path()).remove(0);
        assert_eq!((first.outcome, first.id.as_deref()), (Outcome::Failed, Some("HW-175")));
        assert_eq!(first.detail, "PERC H740P Mini reports a fault (code 0x00002000)");
    }

    #[test]
    fn each_controller_is_checked_and_named_by_its_number() {
        let tree = Tree::new();
        tree.put(0, "controller", "model A\n");
        tree.put(1, "controller", "model B\nstate fault\n");
        let names: Vec<_> = run(tree.path()).iter().map(|c| c.name.clone()).collect();
        assert!(names.contains(&"raid0 controller".to_string()));
        assert!(names.contains(&"raid1 controller".to_string()));
        assert_eq!(names.iter().filter(|n| n.ends_with("battery")).count(), 2);
    }
}
