// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The RAID controllers the kernel shows in `/kernel/raid` (ADR 0664):
//! what `storage raid` says of them, what `diskhealth` reads of the disks
//! behind them, what `hardware check` judges and what metricsd watches.
//!
//! The driver writes a folder for each controller, `/kernel/raid/<n>/`,
//! with the files `controller` and `battery`, whose lines are `key value`,
//! and `volumes`, `disks` and `events`, whose lines are records of
//! `key=value` fields apart by tabs (radix-sysfs(5)). [`read`] parses them
//! into [`Controller`]s, which serialize as the JSON of `storage raid
//! --json` and of admind's `storage.raid` route.
//!
//! A key this code does not know is ignored, as are the fields in any
//! order, and a line that does not make a record (no target, no ID, no
//! sequence number) is left out: a file the driver writes anew on each
//! read is never refused whole for one bad line. A value that is `-`, or
//! missing, or does not read as the number it should be, is `None`.
//!
//! The binary `health/<device id>` files are read by diskhealth; [`member`]
//! finds the disk a name such as `raid0:bay4` stands for.
//!
//! When something here looks wrong, read the file itself: `cat
//! /kernel/raid/0/disks`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::str::FromStr;

use serde::Serialize;

use crate::json;

/// Where the driver shows its controllers.
pub(crate) const ROOT: &str = "/kernel/raid";

/// The most events a controller's answer holds, the newest.
pub(crate) const EVENTS_KEPT: usize = 50;

/// Seconds from 1970 to 2000, which the controller's clock counts from.
const CONTROLLER_EPOCH: i64 = 946_684_800;

/// What a controller says of itself, from its `controller` file. A field
/// the controller does not give is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Info {
    pub(crate) model: Option<String>,
    pub(crate) serial: Option<String>,
    pub(crate) package: Option<String>,
    pub(crate) firmware: Option<String>,
    pub(crate) interface: Option<String>,
    pub(crate) chip: Option<String>,
    pub(crate) pci: Option<String>,
    pub(crate) numa: Option<u32>,
    pub(crate) cache: Option<u64>,
    pub(crate) mode: Option<String>,
    pub(crate) battery: Option<String>,
    pub(crate) temperature: Option<i64>,
    pub(crate) state: Option<String>,
    pub(crate) fault: Option<String>,
    pub(crate) volumes: Option<u64>,
    pub(crate) degraded: Option<u64>,
    pub(crate) offline: Option<u64>,
    pub(crate) disks: Option<u64>,
    pub(crate) critical: Option<u64>,
    pub(crate) failed: Option<u64>,
    pub(crate) commands: Option<u64>,
}

/// One volume, a line of `volumes`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Volume {
    pub(crate) target: u32,
    pub(crate) name: Option<String>,
    pub(crate) level: Option<String>,
    pub(crate) size: Option<u64>,
    pub(crate) stripe: Option<u64>,
    pub(crate) state: Option<String>,
    pub(crate) write: Option<String>,
    pub(crate) default_write: Option<String>,
    pub(crate) read: Option<String>,
    pub(crate) disk_cache: Option<String>,
    /// The device IDs of its disks, in the order of its spans.
    pub(crate) disks: Vec<u32>,
    /// The `sd` name it has as a disk of the system.
    pub(crate) os: Option<String>,
    pub(crate) task: Option<String>,
    pub(crate) percent: Option<u8>,
    pub(crate) elapsed: Option<u64>,
}

/// One physical disk, a line of `disks`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Disk {
    pub(crate) id: u32,
    pub(crate) enclosure: Option<u32>,
    pub(crate) index: Option<u32>,
    pub(crate) slot: Option<u32>,
    /// Where it sits, in words: "Bay 3", "Bay 3, backplane 2" or "Slot 3".
    pub(crate) bay: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) interface: Option<String>,
    pub(crate) media: Option<String>,
    pub(crate) vendor: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) serial: Option<String>,
    pub(crate) revision: Option<String>,
    pub(crate) size: Option<u64>,
    pub(crate) raw_size: Option<u64>,
    pub(crate) media_errors: Option<u64>,
    pub(crate) other_errors: Option<u64>,
    pub(crate) predictive: Option<u64>,
    pub(crate) smart_alert: bool,
    pub(crate) temperature: Option<i64>,
    /// The target of the volume it is in.
    pub(crate) volume: Option<u32>,
    pub(crate) spare: Option<String>,
    /// The `sd` name it has when it is passed through to the system.
    pub(crate) os: Option<String>,
    pub(crate) task: Option<String>,
    pub(crate) percent: Option<u8>,
    pub(crate) elapsed: Option<u64>,
}

/// The cache battery or capacitor module, from `battery`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Battery {
    #[serde(rename = "type")]
    pub(crate) kind: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) charge: Option<u64>,
    pub(crate) temperature: Option<i64>,
    pub(crate) voltage: Option<i64>,
    pub(crate) current: Option<i64>,
    pub(crate) learn: Option<String>,
    /// Whether a volume set to write back writes through now, for the
    /// battery's sake.
    pub(crate) write_through: bool,
    pub(crate) flags: Vec<String>,
}

/// One event of the controller's log, a line of `events`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Event {
    pub(crate) seq: u64,
    /// When it happened, RFC 3339 in UTC; none for an event the controller
    /// stamped with the seconds since its own start.
    pub(crate) time: Option<String>,
    pub(crate) boot_relative: Option<u64>,
    pub(crate) class: Option<String>,
    pub(crate) locale: Vec<String>,
    pub(crate) code: Option<u32>,
    pub(crate) volume: Option<u32>,
    pub(crate) disk: Option<u32>,
    pub(crate) enclosure: Option<u32>,
    pub(crate) slot: Option<u32>,
    pub(crate) bay: Option<String>,
    pub(crate) percent: Option<u8>,
    pub(crate) text: Option<String>,
    /// The same moment in seconds since 1970, for the text of `storage
    /// raid events`.
    #[serde(skip)]
    pub(crate) at: Option<i64>,
}

/// A controller with all the kernel says of it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Controller {
    pub(crate) n: u32,
    pub(crate) controller: Info,
    pub(crate) volumes: Vec<Volume>,
    pub(crate) disks: Vec<Disk>,
    /// None for a controller without a battery.
    pub(crate) battery: Option<Battery>,
    /// The newest [`EVENTS_KEPT`], the oldest first.
    pub(crate) events: Vec<Event>,
}

/// What `storage raid --json` and `storage.raid` give.
#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub(crate) controllers: Vec<Controller>,
}

/// The fields of one line, by key.
struct Fields<'a>(HashMap<&'a str, &'a str>);

impl<'a> Fields<'a> {
    /// The fields of a record line: `key=value` apart by tabs. A piece
    /// with no `=` is left out; the value is what follows the first one.
    fn of_record(line: &'a str) -> Fields<'a> {
        Fields(line.split('\t').filter_map(|piece| piece.split_once('=')).collect())
    }

    /// The lines of a key file: a key, a space, and the value to the end
    /// of the line, which may be empty.
    fn of_keys(text: &'a str) -> Fields<'a> {
        let lines = text.lines().map(|line| line.split_once(' ').unwrap_or((line, "")));
        Fields(lines.collect())
    }

    /// The value of `key`, none when the kernel does not know it (`-`)
    /// or does not give it.
    fn text(&self, key: &str) -> Option<String> {
        self.0.get(key).filter(|value| **value != "-").map(|value| (*value).to_string())
    }

    fn number<T: FromStr>(&self, key: &str) -> Option<T> {
        self.0.get(key)?.parse().ok()
    }

    /// A code in hex, `0x0051`.
    fn hex(&self, key: &str) -> Option<u32> {
        u32::from_str_radix(self.0.get(key)?.strip_prefix("0x")?, 16).ok()
    }

    fn yes(&self, key: &str) -> bool {
        self.0.get(key) == Some(&"yes")
    }

    /// The comma-separated values of `key`, none for `-`.
    fn list(&self, key: &str) -> Vec<String> {
        let Some(value) = self.text(key) else { return Vec::new() };
        value.split(',').filter(|word| !word.is_empty()).map(str::to_string).collect()
    }
}

fn parse_info(text: &str) -> Info {
    let fields = Fields::of_keys(text);
    Info {
        model: fields.text("model"),
        serial: fields.text("serial"),
        package: fields.text("package"),
        firmware: fields.text("firmware"),
        interface: fields.text("interface"),
        chip: fields.text("chip"),
        pci: fields.text("pci"),
        numa: fields.number("numa"),
        cache: fields.number("cache"),
        mode: fields.text("mode"),
        battery: fields.text("battery"),
        temperature: fields.number("temperature"),
        state: fields.text("state"),
        fault: fields.text("fault"),
        volumes: fields.number("volumes"),
        degraded: fields.number("degraded"),
        offline: fields.number("offline"),
        disks: fields.number("disks"),
        critical: fields.number("critical"),
        failed: fields.number("failed"),
        commands: fields.number("commands"),
    }
}

fn parse_volumes(text: &str) -> Vec<Volume> {
    text.lines()
        .filter_map(|line| {
            let fields = Fields::of_record(line);
            Some(Volume {
                target: fields.number("target")?,
                name: fields.text("name"),
                level: fields.text("level"),
                size: fields.number("size"),
                stripe: fields.number("stripe"),
                state: fields.text("state"),
                write: fields.text("write"),
                default_write: fields.text("default-write"),
                read: fields.text("read"),
                disk_cache: fields.text("disk-cache"),
                disks: fields.list("disks").iter().filter_map(|id| id.parse().ok()).collect(),
                os: fields.text("os"),
                task: fields.text("task"),
                percent: fields.number("percent"),
                elapsed: fields.number("elapsed"),
            })
        })
        .collect()
}

/// How a place in an enclosure is said: by its slot when the controller's
/// disks are in one enclosure, which is the machine's backplane, with the
/// backplane's number when they are in more, and as a slot when the disk
/// is attached directly.
fn place(enclosures: usize, index: Option<u32>, slot: Option<u32>, direct: bool) -> Option<String> {
    let slot = slot?;
    if direct {
        return Some(format!("Slot {slot}"));
    }
    match index {
        Some(index) if enclosures > 1 => Some(format!("Bay {slot}, backplane {}", index + 1)),
        _ => Some(format!("Bay {slot}")),
    }
}

fn parse_disks(text: &str) -> Vec<Disk> {
    let mut disks: Vec<Disk> = text
        .lines()
        .filter_map(|line| {
            let fields = Fields::of_record(line);
            Some(Disk {
                id: fields.number("id")?,
                enclosure: fields.number("enclosure"),
                index: fields.number("index"),
                slot: fields.number("slot"),
                bay: None,
                state: fields.text("state"),
                interface: fields.text("interface"),
                media: fields.text("media"),
                vendor: fields.text("vendor"),
                model: fields.text("model"),
                serial: fields.text("serial"),
                revision: fields.text("revision"),
                size: fields.number("size"),
                raw_size: fields.number("raw-size"),
                media_errors: fields.number("media-errors"),
                other_errors: fields.number("other-errors"),
                predictive: fields.number("predictive"),
                smart_alert: fields.yes("smart-alert"),
                temperature: fields.number("temperature"),
                volume: fields.number("volume"),
                spare: fields.text("spare"),
                os: fields.text("os"),
                task: fields.text("task"),
                percent: fields.number("percent"),
                elapsed: fields.number("elapsed"),
            })
        })
        .collect();
    disks.sort_by_key(|disk| disk.id);
    let enclosures = enclosures_of(&disks);
    for disk in &mut disks {
        disk.bay = place(enclosures, disk.index, disk.slot, disk.enclosure.is_none());
    }
    disks
}

/// How many enclosures the disks are in.
fn enclosures_of(disks: &[Disk]) -> usize {
    disks.iter().filter_map(|disk| disk.enclosure).collect::<HashSet<_>>().len()
}

fn parse_battery(text: &str) -> Battery {
    let fields = Fields::of_keys(text);
    Battery {
        kind: fields.text("type"),
        state: fields.text("state"),
        charge: fields.number("charge"),
        temperature: fields.number("temperature"),
        voltage: fields.number("voltage"),
        current: fields.number("current"),
        learn: fields.text("learn"),
        write_through: fields.yes("write-through"),
        flags: fields.list("flags"),
    }
}

/// The events of `text`, the newest [`EVENTS_KEPT`], oldest first. `disks`
/// name the bays of the events that name a disk.
fn parse_events(text: &str, disks: &[Disk]) -> Vec<Event> {
    let enclosures = enclosures_of(disks);
    let mut events: Vec<Event> = text
        .lines()
        .filter_map(|line| {
            let fields = Fields::of_record(line);
            let stamp = fields.text("time");
            let boot_relative =
                stamp.as_deref().and_then(|stamp| stamp.strip_prefix("boot+")?.parse().ok());
            let at = match (&stamp, boot_relative) {
                (Some(stamp), None) => {
                    stamp.parse::<i64>().ok().map(|seconds| seconds + CONTROLLER_EPOCH)
                }
                _ => None,
            };
            let (disk, enclosure, slot) =
                (fields.number("disk"), fields.number("enclosure"), fields.number("slot"));
            // A disk the controller still lists says where it is itself.
            let known = disks.iter().find(|known| Some(known.id) == disk);
            let bay = match known {
                Some(known) => known.bay.clone(),
                None => place(enclosures, enclosure, slot, enclosure.is_none()),
            };
            Some(Event {
                seq: fields.number("seq")?,
                time: at.map(json::time),
                boot_relative,
                class: fields.text("class"),
                locale: fields.list("locale"),
                code: fields.hex("code"),
                volume: fields.number("volume"),
                disk,
                enclosure,
                slot,
                bay,
                percent: fields.number("percent"),
                text: fields.text("text"),
                at,
            })
        })
        .collect();
    let extra = events.len().saturating_sub(EVENTS_KEPT);
    events.drain(..extra);
    events
}

/// A file of controller `dir`, empty when it is not there: a file the
/// driver does not write is as good as one with nothing in it.
fn file_of(dir: &Path, name: &str) -> Option<String> {
    let bytes = fs::read(dir.join(name)).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_controller(root: &Path, n: u32) -> Controller {
    let dir = root.join(n.to_string());
    let text = |name: &str| file_of(&dir, name).unwrap_or_default();
    let disks = parse_disks(&text("disks"));
    let events = parse_events(&text("events"), &disks);
    Controller {
        n,
        controller: parse_info(&text("controller")),
        volumes: parse_volumes(&text("volumes")),
        battery: file_of(&dir, "battery").map(|text| parse_battery(&text)),
        disks,
        events,
    }
}

/// The controllers in `root`, by number; none when the folder is not
/// there, as on a machine without one.
pub(crate) fn read(root: &Path) -> Vec<Controller> {
    let Ok(entries) = fs::read_dir(root) else { return Vec::new() };
    let mut numbers: Vec<u32> =
        entries.flatten().filter_map(|entry| entry.file_name().to_str()?.parse().ok()).collect();
    numbers.sort_unstable();
    numbers.into_iter().map(|n| read_controller(root, n)).collect()
}

/// What `storage raid` says of the machine.
pub(crate) fn report(root: &Path) -> Report {
    Report { controllers: read(root) }
}

impl Volume {
    /// Its level in words: `raid1` is "RAID 1", `raid1e` "RAID 1E".
    pub(crate) fn level_words(&self) -> String {
        match self.level.as_deref().and_then(|level| level.strip_prefix("raid")) {
            Some(number) if !number.is_empty() => format!("RAID {}", number.to_uppercase()),
            _ => "RAID".to_string(),
        }
    }

    /// Whether the volume works with all its disks.
    pub(crate) fn is_optimal(&self) -> bool {
        self.state.as_deref() == Some("optimal")
    }
}

impl Disk {
    /// Whether the controller has given the disk up.
    pub(crate) fn has_failed(&self) -> bool {
        matches!(self.state.as_deref(), Some("failed" | "unconfigured-bad"))
    }

    /// Whether the disk or the controller expects the disk to fail.
    pub(crate) fn is_predicted_to_fail(&self) -> bool {
        self.smart_alert || self.predictive.is_some_and(|count| count > 0)
    }

    /// What the disk is called in `diskhealth`'s list: `bay4`, `bay4-2`
    /// for the fourth bay of the second backplane, or `slot4`.
    pub(crate) fn member_word(&self) -> Option<String> {
        let bay = self.bay.as_deref()?.to_lowercase().replace(", backplane ", "-");
        Some(bay.replace(' ', ""))
    }
}

impl Battery {
    /// Whether the battery cannot hold the cache: it failed or is gone.
    pub(crate) fn has_failed(&self) -> bool {
        matches!(self.state.as_deref(), Some("failed" | "missing"))
    }

    /// Whether it is worn out, or its test of its own capacity failed.
    pub(crate) fn is_worn(&self) -> bool {
        self.state.as_deref() == Some("degraded")
            || matches!(self.learn.as_deref(), Some("failed" | "timed-out"))
    }
}

impl Controller {
    /// Whether the controller reports a fault of its own.
    pub(crate) fn is_in_fault(&self) -> bool {
        self.controller.state.as_deref() == Some("fault")
    }

    /// The name by which `diskhealth` lists a disk of this controller that
    /// the system does not see as a disk of its own: `raid0:bay4`.
    pub(crate) fn member_name(&self, disk: &Disk) -> Option<String> {
        Some(format!("raid{}:{}", self.n, disk.member_word()?))
    }

    /// The disks `diskhealth` lists: those behind the controller. A disk
    /// passed through is a disk of the system, listed by its own name.
    pub(crate) fn members(&self) -> impl Iterator<Item = &Disk> {
        self.disks.iter().filter(|disk| disk.os.is_none())
    }
}

/// The controller and the disk that `name`, such as `raid0:bay4`, stands
/// for in `root`.
pub(crate) fn member(root: &Path, name: &str) -> Option<(Controller, Disk)> {
    let (controller, _) = name.strip_prefix("raid")?.split_once(':')?;
    let controller = read_controller(root, controller.parse().ok()?);
    let disk = controller
        .members()
        .find(|disk| controller.member_name(disk).as_deref() == Some(name))?
        .clone();
    Some((controller, disk))
}

/// Whether `name` is of the form a member disk's is: no disk of the
/// kernel's has a colon in its name.
pub(crate) fn is_member_name(name: &str) -> bool {
    name.starts_with("raid") && name.contains(':')
}

/// The file the driver answers a disk's health from: 1032 bytes, as
/// radix-block(2)'s GET_HEALTH writes them.
pub(crate) fn health_path(root: &Path, controller: u32, disk: u32) -> std::path::PathBuf {
    root.join(controller.to_string()).join("health").join(disk.to_string())
}

#[cfg(test)]
pub(crate) mod fixture {
    //! A folder of the files the driver writes, in a place of its own for
    //! each call, as the tests run at once.

    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    pub(crate) struct Tree(PathBuf);

    impl Tree {
        pub(crate) fn new() -> Tree {
            static CALLS: AtomicUsize = AtomicUsize::new(0);
            let call = CALLS.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("raid-{}-{call}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Tree(dir)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }

        /// Writes the file `name` of controller `n`.
        pub(crate) fn put(&self, n: u32, name: &str, text: &str) {
            let path = self.0.join(n.to_string()).join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        /// A PERC H740P Mini with a RAID 1 of two disks, a solid state
        /// disk passed through and a third disk that is rebuilding.
        pub(crate) fn h740p(&self) {
            self.put(0, "controller", CONTROLLER);
            self.put(0, "volumes", VOLUMES);
            self.put(0, "disks", DISKS);
            self.put(0, "battery", BATTERY);
            self.put(0, "events", EVENTS);
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) const CONTROLLER: &str = "\
model PERC H740P Mini
serial 1A2B3C4D
package 51.13.0-3485
firmware 5.130.00-2780
interface fusion
chip SAS3508
pci 0000:3b:00.0
numa 0
cache 8589934592
mode mixed
battery cachevault
temperature 56
state operational
fault -
volumes 1
degraded 0
offline 0
disks 3
critical 0
failed 0
commands 256
unknown-key some value
";

    pub(crate) const VOLUMES: &str = "target=0\tname=-\tlevel=raid1\tsize=599550590976\t\
stripe=65536\tstate=optimal\twrite=back\tdefault-write=back\tread=ahead\tdisk-cache=default\t\
disks=0,1\tos=sda\ttask=none\tpercent=-\telapsed=-\tfuture=1\n";

    pub(crate) const DISKS: &str = "\
id=0\tenclosure=32\tindex=0\tslot=0\tstate=online\tinterface=sas\tmedia=hdd\tvendor=SEAGATE\t\
model=ST600MM0009\tserial=S0M0001\trevision=TT31\tsize=599550590976\traw-size=600127266816\t\
media-errors=0\tother-errors=0\tpredictive=0\tsmart-alert=no\ttemperature=31\tvolume=0\t\
spare=-\tos=-\ttask=none\tpercent=-\telapsed=-
id=1\tenclosure=32\tindex=0\tslot=1\tstate=online\tinterface=sas\tmedia=hdd\tvendor=SEAGATE\t\
model=ST600MM0009\tserial=S0M0002\trevision=TT31\tsize=599550590976\traw-size=600127266816\t\
media-errors=0\tother-errors=0\tpredictive=0\tsmart-alert=no\ttemperature=32\tvolume=0\t\
spare=-\tos=-\ttask=none\tpercent=-\telapsed=-
id=2\tenclosure=32\tindex=0\tslot=2\tstate=jbod\tinterface=sata\tmedia=ssd\tvendor=ATA\t\
model=INTEL SSDSC2KB480G8\tserial=BTYF0001\trevision=XCV1\tsize=480103981056\t\
raw-size=480103981056\tmedia-errors=0\tother-errors=0\tpredictive=0\tsmart-alert=no\t\
temperature=28\tvolume=-\tspare=-\tos=sdb\ttask=none\tpercent=-\telapsed=-
id=3\tenclosure=32\tindex=0\tslot=3\tstate=rebuilding\tinterface=sas\tmedia=hdd\tvendor=SEAGATE\t\
model=ST600MM0009\tserial=S0M0003\trevision=TT31\tsize=599550590976\traw-size=600127266816\t\
media-errors=0\tother-errors=0\tpredictive=0\tsmart-alert=no\ttemperature=33\tvolume=0\t\
spare=-\tos=-\ttask=rebuild\tpercent=42\telapsed=1800
";

    pub(crate) const BATTERY: &str = "\
type cachevault
state optimal
charge 100
temperature 28
voltage 9450
current 0
learn none
write-through no
flags -
";

    pub(crate) const EVENTS: &str = "\
seq=1233\ttime=boot+95\tclass=info\tlocale=controller\tcode=0x00f7\tvolume=-\tdisk=-\tenclosure=-\tslot=-\tpercent=-\ttext=Host driver is loaded
seq=1234\ttime=848736000\tclass=warning\tlocale=disk\tcode=0x0070\tvolume=-\tdisk=3\tenclosure=0\tslot=3\tpercent=-\ttext=Removed: PD 03(e0x20/s3)
seq=1235\ttime=848736060\tclass=progress\tlocale=disk,volume\tcode=0x0067\tvolume=0\tdisk=3\tenclosure=0\tslot=3\tpercent=12\ttext=Rebuild progress on PD 03 is 12%; a=b
";
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;

    #[test]
    fn a_controller_with_a_volume_and_disks_reads_whole() {
        let tree = Tree::new();
        tree.h740p();
        let found = read(tree.path());
        assert_eq!(found.len(), 1);
        let controller = &found[0];
        assert_eq!(controller.n, 0);
        let info = &controller.controller;
        assert_eq!(info.model.as_deref(), Some("PERC H740P Mini"));
        assert_eq!((info.numa, info.cache, info.temperature), (Some(0), Some(8 << 30), Some(56)));
        assert_eq!((info.fault.as_deref(), info.commands), (None, Some(256)));
        let volume = &controller.volumes[0];
        assert_eq!(volume.name, None, "an unnamed volume is `-`");
        assert_eq!((volume.disks.clone(), volume.stripe), (vec![0, 1], Some(65536)));
        assert_eq!((volume.os.as_deref(), volume.percent), (Some("sda"), None));
        assert_eq!(controller.disks.len(), 4);
        assert_eq!(controller.battery.as_ref().unwrap().voltage, Some(9450));
    }

    #[test]
    fn a_disk_is_named_by_its_bay_and_a_passed_through_one_by_its_os_name() {
        let tree = Tree::new();
        tree.h740p();
        let controller = read(tree.path()).remove(0);
        let bays: Vec<_> = controller.disks.iter().map(|d| d.bay.as_deref().unwrap()).collect();
        assert_eq!(bays, ["Bay 0", "Bay 1", "Bay 2", "Bay 3"]);
        let behind: Vec<_> =
            controller.members().filter_map(|disk| controller.member_name(disk)).collect();
        assert_eq!(behind, ["raid0:bay0", "raid0:bay1", "raid0:bay3"]);
        let found = member(tree.path(), "raid0:bay3").unwrap();
        assert_eq!((found.0.n, found.1.id), (0, 3));
        assert!(member(tree.path(), "raid0:bay2").is_none());
        assert!(member(tree.path(), "raid1:bay0").is_none());
        assert!(member(tree.path(), "sda").is_none());
        assert!(is_member_name("raid0:bay3") && !is_member_name("sda"));
    }

    #[test]
    fn disks_in_more_enclosures_have_their_backplane_in_the_name() {
        let tree = Tree::new();
        tree.put(
            0,
            "disks",
            "id=0\tenclosure=32\tindex=0\tslot=4\tstate=online\n\
             id=9\tenclosure=33\tindex=1\tslot=4\tstate=online\n",
        );
        let controller = read(tree.path()).remove(0);
        let bays: Vec<_> = controller.disks.iter().map(|d| d.bay.as_deref().unwrap()).collect();
        assert_eq!(bays, ["Bay 4, backplane 1", "Bay 4, backplane 2"]);
        assert_eq!(controller.member_name(&controller.disks[1]).unwrap(), "raid0:bay4-2");
    }

    #[test]
    fn a_disk_attached_directly_has_a_slot_and_a_disk_without_one_has_no_bay() {
        let tree = Tree::new();
        tree.put(
            0,
            "disks",
            "id=5\tenclosure=-\tindex=-\tslot=2\tstate=online\nid=6\tstate=online\n",
        );
        let controller = read(tree.path()).remove(0);
        assert_eq!(controller.disks[0].bay.as_deref(), Some("Slot 2"));
        assert_eq!(controller.disks[1].bay, None);
        assert_eq!(controller.member_name(&controller.disks[0]).unwrap(), "raid0:slot2");
        assert_eq!(controller.member_name(&controller.disks[1]), None);
    }

    #[test]
    fn events_have_their_time_from_2000_or_from_the_controllers_start() {
        let tree = Tree::new();
        tree.h740p();
        let events = read(tree.path()).remove(0).events;
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].time, None);
        assert_eq!(events[0].boot_relative, Some(95));
        assert_eq!(events[0].at, None);
        assert_eq!(events[1].time.as_deref(), Some("2026-11-23T08:00:00Z"));
        assert_eq!(events[1].boot_relative, None);
        assert_eq!((events[1].code, events[1].disk), (Some(0x70), Some(3)));
        assert_eq!(events[1].bay.as_deref(), Some("Bay 3"));
        assert_eq!(events[2].locale, ["disk", "volume"]);
        assert_eq!(events[2].percent, Some(12));
        assert_eq!(events[2].text.as_deref(), Some("Rebuild progress on PD 03 is 12%; a=b"));
    }

    #[test]
    fn only_the_newest_events_are_kept() {
        let tree = Tree::new();
        let mut lines = String::new();
        for seq in 1..=60 {
            lines.push_str(&format!("seq={seq}\ttime=boot+{seq}\tclass=info\ttext=x\n"));
        }
        tree.put(0, "events", &lines);
        let events = read(tree.path()).remove(0).events;
        assert_eq!(events.len(), EVENTS_KEPT);
        assert_eq!((events[0].seq, events[EVENTS_KEPT - 1].seq), (11, 60));
    }

    #[test]
    fn lines_that_make_no_record_are_left_out_and_nothing_panics() {
        let tree = Tree::new();
        tree.put(0, "controller", "\u{fffd}\n\n   \nstate\nnuma many\ncache -\n=\n");
        tree.put(0, "volumes", "garbage\ntarget=x\n\t\t\ntarget=3\tsize=big\tdisks=a,,2\n");
        tree.put(0, "disks", "id=\nid=1\tslot=zz\tsmart-alert=maybe\tpercent=300\nslot=4\n");
        tree.put(0, "events", "seq=1\ttime=soon\tcode=51\n\nseq=\ntext=lost\n");
        tree.put(0, "battery", "");
        let controller = read(tree.path()).remove(0);
        assert_eq!(controller.controller, Info { state: Some(String::new()), ..Info::default() });
        assert_eq!(controller.volumes.len(), 1);
        assert_eq!((controller.volumes[0].target, controller.volumes[0].size), (3, None));
        assert_eq!(controller.volumes[0].disks, [2]);
        assert_eq!(controller.disks.len(), 1);
        let disk = &controller.disks[0];
        assert_eq!(
            (disk.slot, disk.smart_alert, disk.percent, disk.bay.clone()),
            (None, false, None, None)
        );
        assert_eq!(controller.events.len(), 1);
        assert_eq!((controller.events[0].time.clone(), controller.events[0].code), (None, None));
        assert_eq!(controller.battery, Some(Battery::default()));
    }

    #[test]
    fn a_folder_that_is_not_there_or_is_not_a_controller_gives_none() {
        let tree = Tree::new();
        assert!(read(&tree.path().join("nowhere")).is_empty());
        fs::create_dir_all(tree.path().join("health")).unwrap();
        assert!(read(tree.path()).is_empty());
        let bare = Tree::new();
        bare.put(1, "controller", "model X\n");
        bare.put(0, "controller", "model Y\n");
        let numbers: Vec<_> = read(bare.path()).iter().map(|c| c.n).collect();
        assert_eq!(numbers, [0, 1]);
        assert_eq!(read(bare.path())[0].battery, None);
    }

    #[test]
    fn what_a_disk_is_in_the_controllers_words() {
        let disk = |state: &str, alert: bool, predictive: u64| Disk {
            state: Some(state.into()),
            smart_alert: alert,
            predictive: Some(predictive),
            ..Disk::default()
        };
        assert!(disk("failed", false, 0).has_failed());
        assert!(disk("unconfigured-bad", false, 0).has_failed());
        assert!(!disk("online", true, 0).has_failed());
        assert!(disk("online", true, 0).is_predicted_to_fail());
        assert!(disk("online", false, 2).is_predicted_to_fail());
        assert!(!disk("online", false, 0).is_predicted_to_fail());
    }

    #[test]
    fn a_battery_that_failed_is_gone_or_is_worn() {
        let battery = |state: &str, learn: &str| Battery {
            state: Some(state.into()),
            learn: Some(learn.into()),
            ..Battery::default()
        };
        assert!(battery("failed", "none").has_failed());
        assert!(battery("missing", "none").has_failed());
        assert!(!battery("optimal", "none").has_failed() && !battery("optimal", "none").is_worn());
        assert!(battery("degraded", "none").is_worn());
        assert!(
            battery("optimal", "failed").is_worn() && battery("optimal", "timed-out").is_worn()
        );
        assert!(!battery("learning", "active").is_worn());
    }

    #[test]
    fn the_document_has_the_files_keys_with_dashes_turned_to_underscores() {
        let tree = Tree::new();
        tree.h740p();
        let line = json::line(&report(tree.path())).unwrap();
        json::assert_golden(&line, include_str!("../tests/json/storage-raid.json"));
        let empty = json::line(&report(&tree.path().join("nowhere"))).unwrap();
        assert_eq!(empty, "{\"format\":1,\"controllers\":[]}");
    }
}
