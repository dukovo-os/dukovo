// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `storage raid`: the RAID controllers of the machine, their volumes,
//! the disks behind them and their batteries, and `storage raid events`,
//! what each controller has logged lately (ADR 0664).
//!
//! Anyone may run it. It reads `/kernel/raid` (the files are in
//! radix-sysfs(5), and [`crate::raid`] parses them), which the driver
//! keeps from the controller's last answers, so that it never waits on
//! the controller. The text follows the web console's Storage page: the
//! controller, its volumes with what state each is in and any task that
//! runs, the disks by bay, and the battery. States are said in words, and
//! the time a task has left is a guess from how far it is and how long it
//! has run. A machine without a controller is told so, and the exit
//! status is 0. `--json` gives the same as admind's `storage.raid` route.
//!
//! When a state looks wrong, read the file the line comes from: `cat
//! /kernel/raid/0/volumes`.

use std::path::Path;

use super::show::lined;
use crate::json;
use crate::raid::{self, Battery, Controller, Disk, Event, Volume};
use crate::util::{self, Fail};

/// The line for a machine that has no controller.
const NONE_FOUND: &str = "No RAID controller was found.";

/// How long a task has left, said roughly, from how far it is (a percent
/// from 1 to 99) and the seconds it has run; none when that says nothing.
fn time_left(percent: Option<u8>, elapsed: Option<u64>) -> Option<String> {
    let (percent, elapsed) = (u64::from(percent?), elapsed?);
    if !(1..100).contains(&percent) || elapsed == 0 {
        return None;
    }
    let seconds = elapsed * (100 - percent) / percent;
    let (hours, minutes) = (seconds / 3600, (seconds % 3600).div_ceil(60));
    let (hours, minutes) = if minutes == 60 { (hours + 1, 0) } else { (hours, minutes) };
    Some(match (hours, minutes) {
        (0, 0) => "less than a minute".to_string(),
        (0, minutes) => format!("{minutes} min"),
        (hours, _) if hours >= 48 => format!("{} days", hours.div_ceil(24)),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    })
}

/// The progress of a task: "42%, about 1 h 10 min left", or the percent
/// alone when the time cannot be guessed, or nothing.
fn progress(percent: Option<u8>, elapsed: Option<u64>) -> String {
    match (percent, time_left(percent, elapsed)) {
        (Some(percent), Some(left)) => format!(", {percent}%, about {left} left"),
        (Some(percent), None) => format!(", {percent}%"),
        _ => String::new(),
    }
}

/// Where a disk is, for a line: its bay, else "disk" and its number.
fn place(disk: &Disk) -> String {
    disk.bay.clone().unwrap_or_else(|| format!("Disk {}", disk.id))
}

/// A volume's state in words, with how many of its disks are gone.
fn volume_state(volume: &Volume, disks: &[Disk]) -> String {
    let absent = volume.disks.iter().filter(|id| !disks.iter().any(|d| d.id == **id)).count();
    let failed = disks.iter().filter(|d| volume.disks.contains(&d.id) && d.has_failed()).count();
    let gone = match (absent, failed) {
        (1, 0) => "one disk is missing".to_string(),
        (0, 1) => "one disk has failed".to_string(),
        (0, 0) => "a disk is missing".to_string(),
        (absent, failed) => format!("{} disks are missing or have failed", absent + failed),
    };
    match volume.state.as_deref() {
        Some("optimal") => "Working normally".to_string(),
        Some("partly-degraded") => {
            format!("Partly degraded: {gone}; the volume still works, with less protection")
        }
        Some("degraded") => format!("Degraded: {gone}"),
        Some("offline") => "Offline: the volume cannot be used".to_string(),
        Some(other) => other.to_string(),
        None => "State not known".to_string(),
    }
}

/// What a task of a volume is, in words that go before its progress.
fn volume_task(task: &str) -> Option<&'static str> {
    match task {
        "check" => Some("Checking the data"),
        "init" => Some("Initializing"),
        "background-init" => Some("Initializing in the background"),
        "reconstruct" => Some("Reconstructing"),
        _ => None,
    }
}

/// The lines under a volume for what runs on it: the task it has, and
/// the rebuild of each of its disks.
fn volume_tasks(volume: &Volume, disks: &[Disk]) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(words) = volume.task.as_deref().and_then(volume_task) {
        lines.push(format!("{words}{}", progress(volume.percent, volume.elapsed)));
    }
    for disk in disks.iter().filter(|disk| disk.volume == Some(volume.target)) {
        let words = match disk.task.as_deref() {
            Some("rebuild") => "Rebuilding",
            Some("copyback") => "Copying back to",
            _ => continue,
        };
        lines.push(format!("{words} {}{}", place(disk), progress(disk.percent, disk.elapsed)));
    }
    lines
}

fn volume_text(volume: &Volume, disks: &[Disk]) -> String {
    let level = volume.level_words();
    let size =
        volume.size.map_or_else(String::new, |size| format!(", {}", util::human(size, false)));
    let name = volume.name.as_deref().filter(|name| !name.is_empty());
    let os = volume.os.as_deref().map_or_else(String::new, |os| format!(", {os}"));
    let mut text = format!(
        "  Volume {}{}, {level}{size}{os}: {}\n",
        volume.target,
        name.map_or_else(String::new, |name| format!(" ({name})")),
        volume_state(volume, disks)
    );
    for line in volume_tasks(volume, disks) {
        text.push_str(&format!("    {line}\n"));
    }
    if volume.write.as_deref() == Some("through") && volume.default_write.as_deref() == Some("back")
    {
        text.push_str("    Writes go straight to disk until the battery charges.\n");
    }
    text
}

/// A disk's state in words, with its task and how far it is.
fn disk_state(disk: &Disk) -> String {
    let state = match disk.state.as_deref() {
        Some("online") => "Online",
        Some("unconfigured-good") => "Not in use",
        Some("unconfigured-bad") => "Bad, not in use",
        Some("hot-spare") => "Hot spare",
        Some("offline") => "Offline",
        Some("failed") => "FAILED",
        Some("rebuilding") => "Rebuilding",
        Some("copyback") => "Copying back",
        Some("jbod") => "Passed through",
        Some(other) => other,
        None => "-",
    };
    let task = match disk.task.as_deref() {
        Some("patrol") => " (patrol read)",
        Some("clear") => " (erasing)",
        _ => "",
    };
    let percent = match (disk.task.as_deref(), disk.percent) {
        (Some("rebuild" | "copyback"), Some(percent)) => format!(" {percent}%"),
        _ => String::new(),
    };
    format!("{state}{percent}{task}")
}

/// What a disk is used for: its volume, the spare it is, or its name for
/// the system.
fn disk_use(disk: &Disk) -> String {
    match (disk.volume, disk.spare.as_deref(), disk.os.as_deref()) {
        (Some(volume), ..) => format!("Volume {volume}"),
        (None, Some(kind), _) => format!("{kind} spare"),
        (None, None, Some(os)) => os.to_string(),
        _ => "-".to_string(),
    }
}

fn disk_kind(disk: &Disk) -> String {
    let interface = disk.interface.as_deref().map(str::to_uppercase);
    let media = disk.media.as_deref().map(str::to_uppercase);
    match (interface, media) {
        (Some(interface), Some(media)) => format!("{interface} {media}"),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => "-".to_string(),
    }
}

fn disk_model(disk: &Disk) -> String {
    let parts: Vec<&str> =
        [disk.vendor.as_deref(), disk.model.as_deref()].into_iter().flatten().collect();
    if parts.is_empty() { "-".to_string() } else { parts.join(" ") }
}

fn disk_notes(disk: &Disk) -> String {
    let mut notes = Vec::new();
    if disk.is_predicted_to_fail() {
        notes.push("predicted to fail");
    }
    if disk.media_errors.is_some_and(|count| count > 0) {
        notes.push("has medium errors");
    }
    notes.join(", ")
}

fn disks_text(disks: &[Disk]) -> String {
    let mut rows = vec![
        ["BAY", "STATE", "TYPE", "SIZE", "MODEL", "TEMP", "USE", "NOTES"]
            .map(str::to_string)
            .to_vec(),
    ];
    for disk in disks {
        rows.push(vec![
            place(disk),
            disk_state(disk),
            disk_kind(disk),
            disk.size.map_or_else(|| "-".to_string(), |size| util::human(size, false)),
            disk_model(disk),
            disk.temperature.map_or_else(|| "-".to_string(), |degrees| format!("{degrees} °C")),
            disk_use(disk),
            disk_notes(disk),
        ]);
    }
    let table = lined(&rows, &[false, false, false, true, false, true, false, false]);
    let mut indented = String::new();
    for line in table.lines() {
        indented.push_str(&format!("  {line}\n"));
    }
    indented
}

fn battery_state(battery: &Battery) -> String {
    let state = match battery.state.as_deref() {
        Some("optimal") => "working normally",
        Some("charging") => "charging",
        Some("discharging") => "running the cache from its charge",
        Some("learning") => "testing its capacity",
        Some("degraded") => "worn: replace it soon",
        Some("failed") => "FAILED: replace it",
        Some("missing") => "MISSING: it is not fitted or not found",
        Some(other) => other,
        None => "state not known",
    };
    let mut text = state.to_string();
    if let Some(charge) = battery.charge {
        text.push_str(&format!(", {charge}% charged"));
    }
    if let Some(degrees) = battery.temperature {
        text.push_str(&format!(", {degrees} °C"));
    }
    match battery.learn.as_deref() {
        Some("failed") => text.push_str(". Its last test of its capacity failed"),
        Some("timed-out") => text.push_str(". Its last test of its capacity ran out of time"),
        _ => {}
    }
    text
}

fn battery_text(controller: &Controller) -> String {
    let Some(battery) = &controller.battery else { return "Battery: none\n".to_string() };
    let kind = match battery.kind.as_deref() {
        Some("cachevault") => "Cache vault",
        _ => "Battery",
    };
    let mut text = format!("{kind}: {}\n", battery_state(battery));
    if battery.write_through {
        text.push_str("  Writes go straight to disk until the battery charges.\n");
    }
    text
}

fn controller_state(controller: &Controller) -> String {
    let info = &controller.controller;
    match (info.state.as_deref(), info.fault.as_deref()) {
        (Some("fault"), Some(code)) => {
            format!("In fault: the controller reports an error (code {code})")
        }
        (Some("fault"), None) => "In fault: the controller reports an error".to_string(),
        _ => "Working normally".to_string(),
    }
}

fn controller_text(controller: &Controller) -> String {
    let info = &controller.controller;
    let mut parts = vec![info.model.clone().unwrap_or_else(|| "RAID controller".to_string())];
    parts.extend(info.firmware.as_ref().or(info.package.as_ref()).map(|v| format!("firmware {v}")));
    parts.extend(info.cache.map(|bytes| format!("{} cache", util::human(bytes, false))));
    parts.extend(info.temperature.map(|degrees| format!("{degrees} °C")));
    let mut text = format!("Controller {}: {}\n", controller.n, parts.join(", "));
    text.push_str(&format!("  {}\n", controller_state(controller)));
    if !controller.volumes.is_empty() {
        text.push_str("\nVolumes\n");
        for volume in &controller.volumes {
            text.push_str(&volume_text(volume, &controller.disks));
        }
    }
    if !controller.disks.is_empty() {
        text.push_str("\nDisks\n");
        text.push_str(&disks_text(&controller.disks));
    }
    text.push('\n');
    text.push_str(&battery_text(controller));
    text
}

/// What `storage raid` says of `controllers`.
fn text(controllers: &[Controller]) -> String {
    if controllers.is_empty() {
        return format!("{NONE_FOUND}\n");
    }
    controllers.iter().map(controller_text).collect::<Vec<_>>().join("\n")
}

/// When an event happened, in local time, or how long after the
/// controller started.
fn event_time(event: &Event) -> String {
    match (event.at, event.boot_relative) {
        (Some(at), _) => util::local_time(at, "%F %T"),
        (None, Some(seconds)) => format!("start +{}", util::clock_time(seconds)),
        (None, None) => "-".to_string(),
    }
}

fn event_line(event: &Event) -> String {
    let class = event.class.as_deref().unwrap_or("-");
    format!("{}  {class:<8} {}\n", event_time(event), event.text.as_deref().unwrap_or(""))
}

/// What `storage raid events` says of `controllers`: the newest events of
/// each, the newest last.
fn events_text(controllers: &[Controller]) -> String {
    if controllers.is_empty() {
        return format!("{NONE_FOUND}\n");
    }
    let mut out = String::new();
    for controller in controllers {
        if controllers.len() > 1 {
            out.push_str(&format!("Controller {}\n", controller.n));
        }
        if controller.events.is_empty() {
            out.push_str("No events.\n");
        }
        out.extend(controller.events.iter().map(event_line));
    }
    out
}

/// `storage raid [--json]`.
pub fn run(json: bool) -> Result<i32, Fail> {
    show(Path::new(raid::ROOT), json)
}

/// `storage raid events`.
pub fn run_events() -> Result<i32, Fail> {
    print!("{}", events_text(&raid::read(Path::new(raid::ROOT))));
    Ok(0)
}

fn show(root: &Path, as_json: bool) -> Result<i32, Fail> {
    if as_json {
        json::print(&raid::report(root))?;
    } else {
        print!("{}", text(&raid::read(root)));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raid::fixture::{self, Tree};

    fn read(tree: &Tree) -> Vec<Controller> {
        raid::read(tree.path())
    }

    #[test]
    fn the_time_a_task_has_left_is_said_roughly() {
        assert_eq!(time_left(Some(50), Some(600)), Some("10 min".into()));
        assert_eq!(time_left(Some(42), Some(3041)), Some("1 h 10 min".into()));
        assert_eq!(time_left(Some(10), Some(3600)), Some("9 h".into()));
        assert_eq!(time_left(Some(1), Some(7200)), Some("9 days".into()));
        assert_eq!(time_left(Some(99), Some(60)), Some("less than a minute".into()));
        assert_eq!(time_left(Some(0), Some(60)), None);
        assert_eq!(time_left(Some(100), Some(60)), None);
        assert_eq!(time_left(Some(40), Some(0)), None);
        assert_eq!(time_left(None, Some(60)), None);
        assert_eq!(time_left(Some(40), None), None);
    }

    #[test]
    fn a_healthy_machine_reads_in_the_order_the_console_shows() {
        let tree = Tree::new();
        tree.h740p();
        let shown = text(&read(&tree));
        let lines: Vec<&str> = shown.lines().collect();
        assert_eq!(
            lines[0],
            "Controller 0: PERC H740P Mini, firmware 5.130.00-2780, 8.0G cache, 56 °C"
        );
        assert_eq!(lines[1], "  Working normally");
        assert_eq!(lines[3], "Volumes");
        assert_eq!(lines[4], "  Volume 0, RAID 1, 559G, sda: Working normally");
        assert_eq!(lines[5], "    Rebuilding Bay 3, 42%, about 42 min left");
        assert_eq!(lines[7], "Disks");
        assert!(lines[8].starts_with("  BAY"), "{}", lines[8]);
        assert!(lines[9].starts_with("  Bay 0 Online "), "{}", lines[9]);
        assert!(lines[9].contains("SAS HDD") && lines[9].contains("SEAGATE ST600MM0009"));
        assert!(lines[9].ends_with("Volume 0"), "{}", lines[9]);
        assert!(lines[11].contains("Passed through") && lines[11].ends_with("sdb"));
        assert!(lines[12].contains("Rebuilding 42%"), "{}", lines[12]);
        assert_eq!(lines[14], "Cache vault: working normally, 100% charged, 28 °C");
        assert_eq!(lines.len(), 15);
    }

    #[test]
    fn a_degraded_volume_says_how_many_disks_it_lacks() {
        let tree = Tree::new();
        tree.put(
            0,
            "volumes",
            "target=0\tlevel=raid1\tsize=1000000000\tstate=degraded\tdisks=0,1\tos=sda\n",
        );
        tree.put(0, "disks", "id=0\tenclosure=32\tindex=0\tslot=0\tstate=online\tvolume=0\n");
        let shown = text(&read(&tree));
        assert!(
            shown.contains("Volume 0, RAID 1, 954M, sda: Degraded: one disk is missing"),
            "{shown}"
        );
        tree.put(
            0,
            "disks",
            "id=0\tenclosure=32\tindex=0\tslot=0\tstate=online\tvolume=0\nid=1\tenclosure=32\tindex=0\tslot=1\tstate=failed\tvolume=0\n",
        );
        let shown = text(&read(&tree));
        assert!(shown.contains("Degraded: one disk has failed"), "{shown}");
        assert!(shown.contains("FAILED"), "{shown}");
        tree.put(0, "volumes", "target=0\tlevel=raid6\tstate=partly-degraded\tdisks=0,1,2,3\n");
        let shown = text(&read(&tree));
        assert!(shown.contains("Partly degraded: 3 disks are missing or have failed;"), "{shown}");
        tree.put(0, "volumes", "target=0\tlevel=raid5\tstate=offline\n");
        assert!(text(&read(&tree)).contains("Offline: the volume cannot be used"));
    }

    #[test]
    fn a_volume_task_and_the_cache_forced_to_write_through_have_their_lines() {
        let tree = Tree::new();
        tree.put(
            0,
            "volumes",
            "target=1\tname=data\tlevel=raid5\tstate=optimal\twrite=through\tdefault-write=back\t\
             task=check\tpercent=25\telapsed=600\n",
        );
        tree.put(0, "battery", "type battery\nstate charging\ncharge 12\nwrite-through yes\n");
        let shown = text(&read(&tree));
        assert!(shown.contains("Volume 1 (data), RAID 5: Working normally"), "{shown}");
        assert!(shown.contains("    Checking the data, 25%, about 30 min left"), "{shown}");
        assert!(shown.contains("    Writes go straight to disk until the battery charges."));
        assert!(shown.contains("Battery: charging, 12% charged"), "{shown}");
        assert!(shown.ends_with("  Writes go straight to disk until the battery charges.\n"));
    }

    #[test]
    fn a_failed_disk_a_predicted_one_and_a_spare_are_said_so() {
        let tree = Tree::new();
        tree.put(
            0,
            "disks",
            "id=0\tenclosure=32\tindex=0\tslot=0\tstate=failed\tmedia-errors=4\n\
             id=1\tenclosure=32\tindex=0\tslot=1\tstate=online\tsmart-alert=yes\tvolume=0\n\
             id=2\tenclosure=32\tindex=0\tslot=2\tstate=hot-spare\tspare=global\n",
        );
        let shown = text(&read(&tree));
        let row = |bay: &str| {
            shown.lines().find(|l| l.trim_start().starts_with(bay)).unwrap().to_string()
        };
        assert!(row("Bay 0").contains("FAILED") && row("Bay 0").ends_with("has medium errors"));
        assert!(row("Bay 1").ends_with("predicted to fail"));
        assert!(row("Bay 2").contains("Hot spare") && row("Bay 2").contains("global spare"));
    }

    #[test]
    fn a_battery_that_has_failed_is_loud_and_no_battery_is_none() {
        let tree = Tree::new();
        tree.put(0, "battery", "type cachevault\nstate failed\nlearn failed\n");
        let shown = text(&read(&tree));
        assert!(
            shown.contains("Cache vault: FAILED: replace it. Its last test of its capacity failed"),
            "{shown}"
        );
        let bare = Tree::new();
        bare.put(0, "controller", "model X\nstate fault\nfault 0x1234\n");
        let shown = text(&read(&bare));
        assert!(
            shown.contains("  In fault: the controller reports an error (code 0x1234)"),
            "{shown}"
        );
        assert!(shown.ends_with("Battery: none\n"), "{shown}");
    }

    #[test]
    fn a_machine_without_a_controller_says_so_in_one_line() {
        let tree = Tree::new();
        assert_eq!(text(&read(&tree)), "No RAID controller was found.\n");
        assert_eq!(events_text(&read(&tree)), "No RAID controller was found.\n");
        assert_eq!(show(tree.path(), false).unwrap(), 0);
        assert_eq!(show(tree.path(), true).unwrap(), 0);
    }

    #[test]
    fn events_come_oldest_first_with_the_time_of_the_controller_or_its_start() {
        let tree = Tree::new();
        tree.h740p();
        let shown = events_text(&read(&tree));
        let lines: Vec<&str> = shown.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "start +00:01:35  info     Host driver is loaded");
        assert!(lines[1].ends_with("  warning  Removed: PD 03(e0x20/s3)"), "{}", lines[1]);
        assert!(
            lines[2].ends_with("progress Rebuild progress on PD 03 is 12%; a=b"),
            "{}",
            lines[2]
        );
        let quiet = Tree::new();
        quiet.put(0, "controller", "model X\n");
        assert_eq!(events_text(&read(&quiet)), "No events.\n");
        quiet.put(1, "controller", "model Y\n");
        assert!(events_text(&read(&quiet)).starts_with("Controller 0\nNo events.\nController 1\n"));
        assert_eq!(fixture::EVENTS.lines().count(), 3);
    }
}
