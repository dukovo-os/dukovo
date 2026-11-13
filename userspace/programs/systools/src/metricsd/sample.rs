// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What metricsd reads from the system (ADR 0484), and the series it makes
//! of two readings.
//!
//! A [`Reader`] reads everything once into a [`Snapshot`]: the CPUs' times,
//! the load, memory, each disk's, card's and filesystem's numbers, the
//! temperatures, what each service uses, and each Btrfs volume's missing
//! disks. [`series`] turns the snapshot and the one before it into the
//! history's rows, the names and units of which are in metricsd(8), and
//! the Prometheus endpoint writes a snapshot out as it is. The files read
//! are those of radix-proc(5) and radix-sysfs(5), so that a source that is
//! not there, such as a machine without a sensor, only has no series.
//!
//! A counter that went backwards, because a card or a disk was replaced,
//! gives no value for that step, never a negative one. A service's CPU
//! time is kept by [`ServiceClock`] so that it never goes backwards when
//! one of the service's processes ends.
//!
//! When a series is missing from the history, look first at the files
//! here: `cat /processes/stat` and the others show what was read.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::proc::{self, CpuTimes, Memory, Process};
use crate::sensors::{self, Kind};
use crate::{df, util};

/// The kinds of filesystem in `/processes/mounts` that hold the
/// machine's files, as the kernel names them and, for FAT, as Linux does.
const FILESYSTEMS: &[&str] = &["btrfs", "ext4", "xfs", "fat", "vfat", "ntfs", "exfat"];

/// Where each source is, so that tests can read a tree of their own.
#[derive(Clone, Debug)]
pub struct Sources {
    /// The kernel's `/processes`.
    pub processes: PathBuf,
    /// The kernel's `/kernel`.
    pub kernel: PathBuf,
    /// Where Spark reads the service files, whose names are the services'.
    pub services: PathBuf,
}

impl Sources {
    /// Where the system keeps them.
    pub fn system() -> Sources {
        Sources {
            processes: PathBuf::from(proc::PROC),
            kernel: PathBuf::from("/kernel"),
            services: PathBuf::from("/system/settings/spark/services"),
        }
    }
}

/// A disk's counters, as `/kernel/block/<disk>/stat` has them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Disk {
    pub name: String,
    pub reads: u64,
    pub read_sectors: u64,
    pub writes: u64,
    pub written_sectors: u64,
    /// Milliseconds with a request in flight.
    pub busy_ms: u64,
}

/// A mounted filesystem and its space, in bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filesystem {
    /// What it was mounted from: `vda1`.
    pub device: String,
    /// Its kind as Linux's tools name it: FAT is `vfat`.
    pub fstype: String,
    /// Where it is mounted.
    pub mount: String,
    /// Whether it is mounted read-only, so that freeing space is not
    /// something its owner can do.
    pub read_only: bool,
    pub size: u64,
    pub free: u64,
    /// What a program that is not root may still use.
    pub available: u64,
}

impl Filesystem {
    /// What is in use.
    pub fn used(&self) -> u64 {
        self.size.saturating_sub(self.free)
    }
}

/// A network card's counters, as its `statistics` folder has them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Network {
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub tx_dropped: u64,
}

/// A temperature of a hardware monitor chip, in degrees Celsius, with its
/// limits where the sensor gives them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Temperature {
    /// The chip as `sensors` names it: `k10temp-pci-00c3`.
    pub chip: String,
    /// The sensor's file name: `temp1`.
    pub sensor: String,
    /// Its label, or its file name when it has none.
    pub label: String,
    pub celsius: f64,
    pub max: Option<f64>,
    pub crit: Option<f64>,
}

impl Temperature {
    /// What its series is called: `chip/label`.
    pub fn instance(&self) -> String {
        format!("{}/{}", self.chip, self.label)
    }
}

/// A thermal zone of the firmware's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Zone {
    /// What its series is called: `zone/acpitz`.
    pub instance: String,
    pub celsius: f64,
}

/// What a service's processes use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    /// The CPU time the service has used since metricsd looked first,
    /// that of processes that have ended included.
    pub cpu_ns: u64,
    /// The memory its processes hold now.
    pub memory_bytes: u64,
}

/// A Btrfs volume and its disks, from `/kernel/fs/btrfs/<uuid>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Volume {
    pub uuid: String,
    /// Its label, empty if it has none.
    pub label: String,
    pub disks: u64,
    /// The disks missing or failed, and those that missed changes.
    pub missing: u64,
    pub behind: u64,
}

impl Volume {
    /// What it is called: its label, else its UUID.
    pub fn name(&self) -> &str {
        if self.label.is_empty() { &self.uuid } else { &self.label }
    }
}

/// Everything read at one moment.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// When it was read, on the monotonic clock in nanoseconds, which
    /// rates are over, and in seconds since 1970, which rows are stamped
    /// with.
    pub monotonic_ns: u64,
    pub at: i64,
    /// When the system started, in nanoseconds since 1970.
    pub boot_time_ns: u64,
    /// Each CPU's times, and their sums.
    pub cpus: Vec<(u32, CpuTimes)>,
    pub cpu: CpuTimes,
    /// The load averages over 1, 5 and 15 minutes.
    pub load: [f64; 3],
    pub memory: Memory,
    pub disks: Vec<Disk>,
    pub filesystems: Vec<Filesystem>,
    pub networks: Vec<Network>,
    pub temperatures: Vec<Temperature>,
    pub zones: Vec<Zone>,
    /// The services that have processes now.
    pub services: Vec<Service>,
    pub volumes: Vec<Volume>,
}

/// Reads the system into snapshots, reusing its buffers.
pub struct Reader {
    sources: Sources,
    clock: ServiceClock,
    buffer: Vec<u8>,
}

impl Reader {
    pub fn new(sources: Sources) -> Reader {
        Reader { sources, clock: ServiceClock::default(), buffer: Vec::new() }
    }

    /// Reads the system. A file that is not there or does not read is a
    /// source with nothing in it.
    pub fn read(&mut self) -> Snapshot {
        let monotonic_ns = util::monotonic_ns();
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let processes = self.sources.processes.as_path();
        let kernel = self.sources.kernel.as_path();
        let stat = fs::read(processes.join("stat")).map_or_default(|text| proc::parse_stat(&text));
        let in_text = |name: &str| fs::read(processes.join(name)).unwrap_or_default();
        let names = service_names(&self.sources.services);
        let running = read_processes(processes, &mut self.buffer);
        Snapshot {
            monotonic_ns,
            at,
            boot_time_ns: stat.boot_time_ns,
            cpus: stat.cpus,
            cpu: stat.all,
            load: proc::parse_loadavg(&in_text("loadavg")),
            memory: proc::parse_memory(&in_text("meminfo")),
            disks: read_disks(&kernel.join("block")),
            filesystems: read_filesystems(&in_text("mounts")),
            networks: read_networks(&kernel.join("class/net")),
            temperatures: read_temperatures(&kernel.join("class/hwmon")),
            zones: read_zones(&kernel.join("class/thermal")),
            services: self.clock.count(&running, &names),
            volumes: read_volumes(&kernel.join("fs/btrfs")),
        }
    }
}

/// The CPU time of each service, kept so that it only grows.
///
/// A service's CPU time is that of the processes in its resource domain,
/// and a process that ends takes its time with it, so the sum of the live
/// ones falls whenever a worker exits. The clock instead adds what each
/// process used since it was last seen, and all of a new process's.
#[derive(Default)]
pub struct ServiceClock {
    /// The CPU time each process had when it was last seen.
    seen: HashMap<u64, u64>,
    /// What each service has used, from the processes seen.
    total: HashMap<String, u64>,
}

impl ServiceClock {
    /// Takes in the processes now running, and gives each service in
    /// `services` that has one, by name: its CPU time so far and the
    /// memory its processes hold. A process of any other domain, a
    /// session or an app, is left out.
    pub fn count(&mut self, processes: &[Process], services: &HashSet<String>) -> Vec<Service> {
        let mut seen = HashMap::with_capacity(self.seen.len());
        let mut memory: HashMap<&str, u64> = HashMap::new();
        for process in processes.iter().filter(|p| services.contains(&p.domain)) {
            let cpu = process.cpu_ns();
            // A number now with less than it had was reused by another
            // process: all of its time is new.
            let grew = match self.seen.get(&process.number) {
                Some(&before) if cpu >= before => cpu - before,
                _ => cpu,
            };
            match self.total.get_mut(&process.domain) {
                Some(total) => *total += grew,
                None => {
                    self.total.insert(process.domain.clone(), grew);
                }
            }
            seen.insert(process.number, cpu);
            *memory.entry(&process.domain).or_default() += process.resident_kib * 1024;
        }
        self.seen = seen;
        let mut running: Vec<Service> = memory
            .into_iter()
            .map(|(name, memory_bytes)| Service {
                name: name.to_string(),
                cpu_ns: self.total.get(name).copied().unwrap_or(0),
                memory_bytes,
            })
            .collect();
        running.sort_by(|a, b| a.name.cmp(&b.name));
        running
    }
}

/// The names of the service files in `dir`: the services' names, which
/// are their resource domains'.
fn service_names(dir: &Path) -> HashSet<String> {
    let Ok(entries) = fs::read_dir(dir) else { return HashSet::new() };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|file| file.strip_suffix(".toml").map(str::to_string))
        .collect()
}

/// Every process's status, one pass over `dir`; a process that ends while
/// it is read is left out.
fn read_processes(dir: &Path, buffer: &mut Vec<u8>) -> Vec<Process> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut processes = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_name().to_str().is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        buffer.clear();
        let Ok(mut status) = File::open(entry.path().join("status")) else { continue };
        if status.read_to_end(buffer).is_ok() {
            processes.push(proc::parse_status(buffer));
        }
    }
    processes
}

/// A disk's counters from the text of its `stat`: Linux's fields, of
/// which these are reads, sectors read, writes, sectors written and the
/// milliseconds with a request in flight.
fn parse_disk(name: &str, text: &str) -> Option<Disk> {
    let fields: Vec<u64> =
        text.split_whitespace().map(str::parse).collect::<Result<_, _>>().ok()?;
    let [reads, _, read_sectors, _, writes, _, written_sectors, _, _, busy_ms, ..] = fields[..]
    else {
        return None;
    };
    Some(Disk { name: name.to_string(), reads, read_sectors, writes, written_sectors, busy_ms })
}

/// The disks of `block` that keep a `stat`, which are the ones without a
/// `partition` file: a partition's numbers are in its disk's.
fn read_disks(block: &Path) -> Vec<Disk> {
    let Ok(entries) = fs::read_dir(block) else { return Vec::new() };
    let mut disks: Vec<Disk> = entries
        .flatten()
        .filter(|entry| !entry.path().join("partition").exists())
        .filter_map(|entry| {
            let text = fs::read_to_string(entry.path().join("stat")).ok()?;
            parse_disk(entry.file_name().to_str()?, &text)
        })
        .collect();
    disks.sort_by(|a, b| a.name.cmp(&b.name));
    disks
}

/// The number in file `path`, or 0 where there is none.
fn counter(path: &Path) -> u64 {
    fs::read_to_string(path).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0)
}

/// The cards of `net` that keep statistics.
fn read_networks(net: &Path) -> Vec<Network> {
    let Ok(entries) = fs::read_dir(net) else { return Vec::new() };
    let mut networks: Vec<Network> = entries
        .flatten()
        .filter_map(|entry| {
            let statistics = entry.path().join("statistics");
            let count = |name: &str| counter(&statistics.join(name));
            statistics.is_dir().then(|| Network {
                name: entry.file_name().to_string_lossy().into_owned(),
                rx_bytes: count("rx_bytes"),
                tx_bytes: count("tx_bytes"),
                rx_errors: count("rx_errors"),
                tx_errors: count("tx_errors"),
                rx_dropped: count("rx_dropped"),
                tx_dropped: count("tx_dropped"),
            })
        })
        .collect();
    networks.sort_by(|a, b| a.name.cmp(&b.name));
    networks
}

/// The temperature sensors of the hardware monitors in `hwmon`, those
/// that give a reading and do not say they are faulty.
fn read_temperatures(hwmon: &Path) -> Vec<Temperature> {
    let mut temperatures = Vec::new();
    for chip in sensors::chips(hwmon) {
        for feature in sensors::features(&chip).into_iter().filter(|f| f.kind == Kind::Temp) {
            let faulty = feature.value(&chip, "fault").is_some_and(|fault| fault > 0.0);
            let Some(celsius) = feature.value(&chip, "input").filter(|_| !faulty) else {
                continue;
            };
            temperatures.push(Temperature {
                chip: chip.name(),
                sensor: feature.name(),
                label: feature.label(&chip),
                celsius,
                max: feature.value(&chip, "max"),
                crit: feature.value(&chip, "crit"),
            });
        }
    }
    temperatures
}

/// The thermal zones of `thermal` that read, each called after its type.
/// Machines often have several zones of one type, `acpitz`; those get
/// their number as well, so that a zone is the same series each time.
fn read_zones(thermal: &Path) -> Vec<Zone> {
    let Ok(entries) = fs::read_dir(thermal) else { return Vec::new() };
    let mut found: Vec<(u32, String, f64)> = entries
        .flatten()
        .filter_map(|entry| {
            let number = entry.file_name().to_str()?.strip_prefix("thermal_zone")?.parse().ok()?;
            let kind = fs::read_to_string(entry.path().join("type")).ok()?;
            let millidegrees: f64 =
                fs::read_to_string(entry.path().join("temp")).ok()?.trim().parse().ok()?;
            Some((number, kind.trim().to_string(), millidegrees / 1000.0))
        })
        .collect();
    found.sort_by_key(|(number, _, _)| *number);
    let several = |kind: &str| found.iter().filter(|(_, other, _)| other == kind).count() > 1;
    found
        .iter()
        .map(|(number, kind, celsius)| Zone {
            instance: if several(kind) {
                format!("zone/{kind}-{number}")
            } else {
                format!("zone/{kind}")
            },
            celsius: *celsius,
        })
        .collect()
}

/// The Btrfs volumes of the kernel's `btrfs` folder, one folder each by
/// UUID with `label`, `disks`, `missing` and `behind`; none while the
/// folder is not there.
fn read_volumes(btrfs: &Path) -> Vec<Volume> {
    let Ok(entries) = fs::read_dir(btrfs) else { return Vec::new() };
    let mut volumes: Vec<Volume> = entries
        .flatten()
        .filter_map(|entry| {
            let dir = entry.path();
            let number = |name: &str| fs::read_to_string(dir.join(name)).ok()?.trim().parse().ok();
            Some(Volume {
                uuid: entry.file_name().into_string().ok()?,
                label: fs::read_to_string(dir.join("label")).unwrap_or_default().trim().to_string(),
                disks: number("disks")?,
                missing: number("missing")?,
                behind: number("behind")?,
            })
        })
        .collect();
    volumes.sort_by(|a, b| a.uuid.cmp(&b.uuid));
    volumes
}

/// The filesystems of the mounts in `mounts` that hold files, with their
/// space; a mount whose space cannot be read is left out.
fn read_filesystems(mounts: &[u8]) -> Vec<Filesystem> {
    proc::parse_mounts(mounts)
        .into_iter()
        .filter(|mount| FILESYSTEMS.contains(&mount.kind.as_str()))
        .filter_map(|mount| {
            let space = df::statvfs(&mount.path).ok().filter(|space| space.size > 0)?;
            Some(Filesystem {
                device: mount.source,
                fstype: if mount.kind == "fat" { "vfat".into() } else { mount.kind },
                mount: mount.path.to_string_lossy().into_owned(),
                read_only: mount.read_only,
                size: space.size,
                free: space.free,
                available: space.available,
            })
        })
        .collect()
}

/// `metric{instance}`.
fn named(metric: &str, instance: &str) -> String {
    format!("{metric}{{{instance}}}")
}

/// How much a counter grew from `before` to `after`, or NaN if there is no
/// `before` or it went backwards.
fn delta<T>(before: Option<&T>, after: &T, read: impl Fn(&T) -> u64) -> f64 {
    before.and_then(|before| read(after).checked_sub(read(before))).map_or(f64::NAN, |d| d as f64)
}

/// The history's row for `now`, the snapshot after `before`: each series
/// by name and its value, NaN where it has none. Without a `before`, or
/// with one that is not earlier, a rate has none.
pub fn series(before: Option<&Snapshot>, now: &Snapshot) -> Vec<(String, f32)> {
    let mut row: Vec<(String, f64)> = Vec::new();
    // Over no time, or from nothing, every rate is missing.
    let (before, seconds) = match before {
        Some(b) if now.monotonic_ns > b.monotonic_ns => {
            (Some(b), (now.monotonic_ns - b.monotonic_ns) as f64 / 1e9)
        }
        _ => (None, f64::NAN),
    };
    cpu_rates(before, now, &mut row);
    row.push(("load".into(), now.load[0]));
    memory_levels(&now.memory, &mut row);
    disk_rates(before, now, seconds, &mut row);
    for filesystem in &now.filesystems {
        row.push((named("fs.used", &filesystem.mount), filesystem.used() as f64));
        row.push((named("fs.size", &filesystem.mount), filesystem.size as f64));
    }
    network_rates(before, now, seconds, &mut row);
    for temperature in &now.temperatures {
        row.push((named("temp", &temperature.instance()), temperature.celsius));
    }
    for zone in &now.zones {
        row.push((named("temp", &zone.instance), zone.celsius));
    }
    service_levels(before, now, seconds, &mut row);
    for volume in &now.volumes {
        row.push((named("btrfs.missing", volume.name()), volume.missing as f64));
        row.push((named("btrfs.behind", volume.name()), volume.behind as f64));
    }
    row.into_iter().map(|(name, value)| (name, value as f32)).collect()
}

/// The CPUs' share of their time in programs, in the kernel, and both, as
/// percent of all CPUs.
fn cpu_rates(before: Option<&Snapshot>, now: &Snapshot, row: &mut Vec<(String, f64)>) {
    let grew =
        |read: fn(&CpuTimes) -> u64| before.and_then(|b| read(&now.cpu).checked_sub(read(&b.cpu)));
    let shares = match (grew(|c| c.user_ns), grew(|c| c.system_ns), grew(|c| c.idle_ns)) {
        (Some(user), Some(system), Some(idle)) if user + system + idle > 0 => {
            let all = (user + system + idle) as f64;
            [
                (user + system) as f64 * 100.0 / all,
                user as f64 * 100.0 / all,
                system as f64 * 100.0 / all,
            ]
        }
        _ => [f64::NAN; 3],
    };
    for (name, share) in ["cpu", "cpu.user", "cpu.system"].into_iter().zip(shares) {
        row.push((name.into(), share));
    }
}

/// Memory in bytes: what is in use, what programs could still have, and
/// the swap in use; missing when the kernel's numbers could not be read.
fn memory_levels(memory: &Memory, row: &mut Vec<(String, f64)>) {
    let bytes = |kib: u64| if memory.total == 0 { f64::NAN } else { (kib * 1024) as f64 };
    row.push(("memory.used".into(), bytes(memory.used())));
    row.push(("memory.available".into(), bytes(memory.available())));
    row.push(("swap.used".into(), bytes(memory.swap_used())));
}

/// Each disk's read and write rates in bytes a second, and the share of
/// the step it had a request in flight.
fn disk_rates(
    before: Option<&Snapshot>,
    now: &Snapshot,
    seconds: f64,
    row: &mut Vec<(String, f64)>,
) {
    for disk in &now.disks {
        let earlier = before.and_then(|b| b.disks.iter().find(|d| d.name == disk.name));
        let per_second = |read: fn(&Disk) -> u64| delta(earlier, disk, read) / seconds;
        row.push((named("disk.read", &disk.name), per_second(|d| d.read_sectors) * 512.0));
        row.push((named("disk.write", &disk.name), per_second(|d| d.written_sectors) * 512.0));
        // Milliseconds in each second, as a share of it; a clock's rounding
        // can make a disk busy a little more than all of the step.
        let busy = per_second(|d| d.busy_ms) / 10.0;
        row.push((named("disk.busy", &disk.name), if busy > 100.0 { 100.0 } else { busy }));
    }
}

/// Each card's rates in bytes a second, and the errors and drops in the
/// step. The loopback is no card.
fn network_rates(
    before: Option<&Snapshot>,
    now: &Snapshot,
    seconds: f64,
    row: &mut Vec<(String, f64)>,
) {
    for card in now.networks.iter().filter(|card| card.name != "lo") {
        let earlier = before.and_then(|b| b.networks.iter().find(|n| n.name == card.name));
        let problems = |n: &Network| n.rx_errors + n.tx_errors + n.rx_dropped + n.tx_dropped;
        row.push((named("net.rx", &card.name), delta(earlier, card, |n| n.rx_bytes) / seconds));
        row.push((named("net.tx", &card.name), delta(earlier, card, |n| n.tx_bytes) / seconds));
        row.push((named("net.errors", &card.name), delta(earlier, card, problems)));
    }
}

/// Each running service's CPU, as percent of one CPU, and its memory.
fn service_levels(
    before: Option<&Snapshot>,
    now: &Snapshot,
    seconds: f64,
    row: &mut Vec<(String, f64)>,
) {
    for service in &now.services {
        let earlier = before.and_then(|b| b.services.iter().find(|s| s.name == service.name));
        let cpu = delta(earlier, service, |s| s.cpu_ns) * 100.0 / 1e9 / seconds;
        row.push((named("service.cpu", &service.name), cpu));
        row.push((named("service.memory", &service.name), service.memory_bytes as f64));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of the test's own.
    fn folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("metricsd-sample-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes `text` to `path` below `dir`, making the folders.
    fn put(dir: &Path, path: &str, text: &str) {
        let file = dir.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }

    fn status(number: u64, domain: &str, cpu_ns: u64, resident_kib: u64) -> Process {
        Process {
            number,
            domain: domain.into(),
            user_ns: cpu_ns,
            resident_kib,
            ..Process::default()
        }
    }

    fn names(services: &[&str]) -> HashSet<String> {
        services.iter().map(ToString::to_string).collect()
    }

    fn value(row: &[(String, f32)], name: &str) -> f32 {
        row.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no {name}")).1
    }

    #[test]
    fn a_disks_stat_is_read_in_linuxs_field_order() {
        let stat = "4200 0 190000 5100 800 0 76000 2300 0 9100 7400 12 0 8 3 5 20\n";
        let disk = parse_disk("vda", stat).unwrap();
        assert_eq!(
            disk,
            Disk {
                name: "vda".into(),
                reads: 4200,
                read_sectors: 190_000,
                writes: 800,
                written_sectors: 76_000,
                busy_ms: 9100,
            }
        );
        // The first 11 fields are what an older kernel has.
        assert!(parse_disk("vda", "1 0 2 3 4 0 5 6 0 7 8").is_some());
        assert!(parse_disk("vda", "1 0 2").is_none());
        assert!(parse_disk("vda", "1 0 x 3 4 0 5 6 0 7 8").is_none());
    }

    #[test]
    fn disks_are_the_whole_ones_that_keep_a_stat() {
        let dir = folder("disks");
        let stat = "1 0 2 3 4 0 5 6 0 7 8 0 0 0 0 0 0\n";
        put(&dir, "vdb/stat", stat);
        put(&dir, "vda/stat", stat);
        put(&dir, "vda1/stat", stat);
        put(&dir, "vda1/partition", "1\n");
        put(&dir, "sr0/size", "0\n");
        let names: Vec<String> = read_disks(&dir).into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["vda", "vdb"]);
        assert!(read_disks(&dir.join("missing")).is_empty());
    }

    #[test]
    fn cards_are_read_with_their_counters() {
        let dir = folder("net");
        for (card, bytes) in [("net0", 1000), ("lo", 5)] {
            put(&dir, &format!("{card}/statistics/rx_bytes"), &format!("{bytes}\n"));
            put(&dir, &format!("{card}/statistics/tx_bytes"), "70\n");
            put(&dir, &format!("{card}/statistics/tx_dropped"), "3\n");
        }
        put(&dir, "br0/link", "bridge\n");
        let cards = read_networks(&dir);
        assert_eq!(cards.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["lo", "net0"]);
        assert_eq!(
            cards[1],
            Network {
                name: "net0".into(),
                rx_bytes: 1000,
                tx_bytes: 70,
                tx_dropped: 3,
                ..Network::default()
            }
        );
    }

    #[test]
    fn temperatures_come_with_their_limits_and_zones_are_named_by_type() {
        let dir = folder("temps");
        let hwmon = dir.join("hwmon");
        put(&hwmon, "hwmon0/name", "coretemp\n");
        put(&hwmon, "hwmon0/temp1_label", "Package id 0\n");
        put(&hwmon, "hwmon0/temp1_input", "44500\n");
        put(&hwmon, "hwmon0/temp1_crit", "100000\n");
        put(&hwmon, "hwmon0/temp1_max", "80000\n");
        put(&hwmon, "hwmon1/name", "cros_ec\n");
        put(&hwmon, "hwmon1/temp1_input", "36900\n");
        put(&hwmon, "hwmon1/temp2_input", "0\n");
        put(&hwmon, "hwmon1/temp2_fault", "1\n");
        put(&hwmon, "hwmon1/fan1_input", "1190\n");
        let found = read_temperatures(&hwmon);
        assert_eq!(found.len(), 2, "a faulty sensor and a fan are left out");
        assert_eq!(found[0].instance(), "coretemp-isa-0000/Package id 0");
        assert_eq!(
            (found[0].celsius, found[0].max, found[0].crit),
            (44.5, Some(80.0), Some(100.0))
        );
        assert_eq!(found[1].instance(), "cros_ec-isa-0000/temp1");
        assert_eq!((found[1].sensor.as_str(), found[1].crit), ("temp1", None));

        let thermal = dir.join("thermal");
        put(&thermal, "thermal_zone0/type", "acpitz\n");
        put(&thermal, "thermal_zone0/temp", "27800\n");
        put(&thermal, "thermal_zone1/type", "acpitz\n");
        put(&thermal, "thermal_zone1/temp", "30000\n");
        put(&thermal, "thermal_zone2/type", "pch\n");
        put(&thermal, "thermal_zone2/temp", "41000\n");
        // A zone whose reading fails is not there.
        put(&thermal, "thermal_zone3/type", "bad\n");
        put(&thermal, "cooling_device0/type", "Processor\n");
        let zones = read_zones(&thermal);
        let said: Vec<(&str, f64)> =
            zones.iter().map(|z| (z.instance.as_str(), z.celsius)).collect();
        assert_eq!(said, [("zone/acpitz-0", 27.8), ("zone/acpitz-1", 30.0), ("zone/pch", 41.0)]);
    }

    #[test]
    fn btrfs_volumes_are_read_from_the_kernels_folders() {
        let dir = folder("btrfs");
        let volume = |uuid: &str, label: &str, disks: &str, missing: &str, behind: &str| {
            for (file, text) in
                [("label", label), ("disks", disks), ("missing", missing), ("behind", behind)]
            {
                put(&dir, &format!("{uuid}/{file}"), &format!("{text}\n"));
            }
        };
        volume("b7c1", "data", "2", "1", "0");
        volume("a001", "", "1", "0", "0");
        // A volume whose files do not read is not listed.
        put(&dir, "c9d2/label", "x\n");
        let volumes = read_volumes(&dir);
        assert_eq!(
            volumes,
            [
                Volume {
                    uuid: "a001".into(),
                    label: String::new(),
                    disks: 1,
                    missing: 0,
                    behind: 0
                },
                Volume {
                    uuid: "b7c1".into(),
                    label: "data".into(),
                    disks: 2,
                    missing: 1,
                    behind: 0
                },
            ]
        );
        assert_eq!((volumes[0].name(), volumes[1].name()), ("a001", "data"));
        // While the kernel has no such folder there are no volumes.
        assert!(read_volumes(&dir.join("not-there")).is_empty());
    }

    #[test]
    fn filesystems_are_those_of_a_disk_kind_with_their_space() {
        let here = std::env::temp_dir();
        let mounts = format!(
            "vda1 / btrfs rw label=root\ntmpfs /tmp tmpfs rw\nvda2 /boot fat rw\n\
             sdb1 {} ext4 ro\nproc /processes proc rw\nsdc1 /gone ntfs rw\n",
            here.display()
        );
        let found = read_filesystems(mounts.as_bytes());
        let said: Vec<(&str, &str, &str)> = found
            .iter()
            .map(|f| (f.device.as_str(), f.fstype.as_str(), f.mount.as_str()))
            .collect();
        // /boot and /gone are not there on this machine; / is.
        assert!(said.contains(&("vda1", "btrfs", "/")), "{said:?}");
        assert!(said.contains(&("sdb1", "ext4", here.to_str().unwrap())), "{said:?}");
        assert!(!said.iter().any(|(_, kind, _)| *kind == "tmpfs" || *kind == "proc"));
        // A read-only mount is said to be.
        let read_only = |mount: &str| found.iter().find(|f| f.mount == mount).map(|f| f.read_only);
        assert_eq!(read_only(here.to_str().unwrap()), Some(true));
        assert_eq!(read_only("/"), Some(false));
        let root = found.iter().find(|f| f.mount == "/").unwrap();
        assert!(root.size > 0 && root.used() <= root.size && root.available <= root.size);
    }

    #[test]
    fn a_service_is_the_domain_of_its_processes() {
        let services = names(&["web", "db"]);
        let mut clock = ServiceClock::default();
        let running = clock.count(
            &[
                status(10, "web", 1_000, 4),
                status(11, "web", 500, 6),
                status(12, "db", 7_000, 100),
                status(13, "alice-session", 9_000, 50),
                status(14, "system", 9_000, 50),
            ],
            &services,
        );
        assert_eq!(
            running,
            [
                Service { name: "db".into(), cpu_ns: 7_000, memory_bytes: 100 * 1024 },
                Service { name: "web".into(), cpu_ns: 1_500, memory_bytes: 10 * 1024 },
            ]
        );
    }

    #[test]
    fn a_services_cpu_time_never_falls_when_a_process_ends() {
        let services = names(&["web"]);
        let mut clock = ServiceClock::default();
        clock.count(&[status(10, "web", 1_000, 1), status(11, "web", 400, 1)], &services);
        // Worker 11 ends, 10 used 100 more and a new one, 12, has used 50.
        let running =
            clock.count(&[status(10, "web", 1_100, 1), status(12, "web", 50, 1)], &services);
        assert_eq!(running[0].cpu_ns, 1_000 + 400 + 100 + 50);
        // Its number reused by another process that has used less.
        let running = clock.count(&[status(10, "web", 20, 1)], &services);
        assert_eq!(running[0].cpu_ns, 1_550 + 20);
        // No process now: no service, and its time is there when one comes.
        assert!(clock.count(&[], &services).is_empty());
        let running = clock.count(&[status(30, "web", 5, 1)], &services);
        assert_eq!(running[0].cpu_ns, 1_570 + 5);
    }

    #[test]
    fn the_first_sample_has_no_rates() {
        let now = Snapshot {
            monotonic_ns: 5_000_000_000,
            load: [0.25, 0.0, 0.0],
            disks: vec![Disk { name: "vda".into(), read_sectors: 100, ..Disk::default() }],
            networks: vec![Network { name: "net0".into(), rx_bytes: 9, ..Network::default() }],
            services: vec![Service { name: "web".into(), cpu_ns: 5, memory_bytes: 2048 }],
            ..Snapshot::default()
        };
        let row = series(None, &now);
        assert_eq!(value(&row, "load"), 0.25);
        assert_eq!(value(&row, "service.memory{web}"), 2048.0);
        for name in [
            "cpu",
            "cpu.user",
            "cpu.system",
            "disk.read{vda}",
            "disk.busy{vda}",
            "net.rx{net0}",
            "net.errors{net0}",
            "service.cpu{web}",
        ] {
            assert!(value(&row, name).is_nan(), "{name}");
        }
        // The same when the clock has not moved on.
        assert!(value(&series(Some(&now), &now), "net.rx{net0}").is_nan());
    }

    #[test]
    fn rates_are_over_the_time_between_the_samples() {
        let sample = |at: u64, scale: u64| Snapshot {
            monotonic_ns: at * 1_000_000_000,
            cpu: CpuTimes {
                user_ns: 10 * scale,
                system_ns: 5 * scale,
                idle_ns: 85 * scale,
                switches: 0,
            },
            disks: vec![Disk {
                name: "vda".into(),
                read_sectors: 2_000 * scale,
                written_sectors: 1_000 * scale,
                busy_ms: 500 * scale,
                ..Disk::default()
            }],
            networks: vec![
                Network { name: "lo".into(), rx_bytes: 99, ..Network::default() },
                Network {
                    name: "net0".into(),
                    rx_bytes: 6_000 * scale,
                    tx_bytes: 3_000 * scale,
                    rx_errors: scale,
                    tx_dropped: 2 * scale,
                    ..Network::default()
                },
            ],
            services: vec![Service {
                name: "web".into(),
                cpu_ns: 5_000_000_000 * scale,
                memory_bytes: 7,
            }],
            ..Snapshot::default()
        };
        // Five seconds apart, the counters of the second five times the first's.
        let row = series(Some(&sample(10, 1)), &sample(15, 6));
        assert_eq!(value(&row, "cpu"), 15.0);
        assert_eq!(value(&row, "cpu.user"), 10.0);
        assert_eq!(value(&row, "cpu.system"), 5.0);
        // 5 * 2000 sectors of 512 bytes in 5 seconds.
        assert_eq!(value(&row, "disk.read{vda}"), 2_000.0 * 512.0);
        assert_eq!(value(&row, "disk.write{vda}"), 1_000.0 * 512.0);
        // 2500 ms in 5 seconds: half of it.
        assert_eq!(value(&row, "disk.busy{vda}"), 50.0);
        assert_eq!(value(&row, "net.rx{net0}"), 6_000.0);
        assert_eq!(value(&row, "net.tx{net0}"), 3_000.0);
        // Errors and drops of the step, not a rate: 5 rx errors and 10 drops.
        assert_eq!(value(&row, "net.errors{net0}"), 15.0);
        assert!(!row.iter().any(|(name, _)| name.contains("{lo}")));
        // 25 s of CPU in 5 s of time: five CPUs' worth.
        assert_eq!(value(&row, "service.cpu{web}"), 500.0);
    }

    #[test]
    fn a_counter_that_went_backwards_has_no_value_for_the_step() {
        let sample = |at: u64, read: u64, rx: u64| Snapshot {
            monotonic_ns: at * 1_000_000_000,
            disks: vec![Disk {
                name: "vda".into(),
                read_sectors: read,
                busy_ms: 0,
                ..Disk::default()
            }],
            networks: vec![Network { name: "net0".into(), rx_bytes: rx, ..Network::default() }],
            cpu: CpuTimes { user_ns: read, system_ns: 0, idle_ns: 100, switches: 0 },
            ..Snapshot::default()
        };
        // The disk was replaced by one with fewer sectors read, the card by
        // one with more bytes.
        let row = series(Some(&sample(1, 1000, 10)), &sample(2, 20, 50));
        assert!(value(&row, "disk.read{vda}").is_nan());
        assert!(value(&row, "cpu").is_nan() && value(&row, "cpu.user").is_nan());
        assert_eq!(value(&row, "net.rx{net0}"), 40.0);
        // A card that was not there before has no rate yet.
        let mut later = sample(3, 20, 60);
        later.networks.push(Network { name: "net1".into(), rx_bytes: 5, ..Network::default() });
        let row = series(Some(&sample(2, 20, 50)), &later);
        assert!(value(&row, "net.rx{net1}").is_nan());
        assert_eq!(value(&row, "net.rx{net0}"), 10.0);
    }

    #[test]
    fn levels_are_read_as_they_are() {
        let now = Snapshot {
            memory: Memory { total: 1000, free: 100, cache: 300, swap_total: 500, swap_free: 400 },
            filesystems: vec![Filesystem {
                mount: "/data".into(),
                size: 1_000_000,
                free: 250_000,
                ..Filesystem::default()
            }],
            temperatures: vec![Temperature {
                chip: "k10temp-pci-00c3".into(),
                label: "Tctl".into(),
                celsius: 45.875,
                ..Temperature::default()
            }],
            zones: vec![Zone { instance: "zone/acpitz".into(), celsius: 27.8 }],
            volumes: vec![Volume {
                uuid: "u".into(),
                label: "data".into(),
                disks: 2,
                missing: 1,
                behind: 0,
            }],
            ..Snapshot::default()
        };
        let row = series(None, &now);
        // Used is what is neither free nor cache.
        assert_eq!(value(&row, "memory.used"), 600.0 * 1024.0);
        assert_eq!(value(&row, "memory.available"), 400.0 * 1024.0);
        assert_eq!(value(&row, "swap.used"), 100.0 * 1024.0);
        assert_eq!(value(&row, "fs.used{/data}"), 750_000.0);
        assert_eq!(value(&row, "fs.size{/data}"), 1_000_000.0);
        assert_eq!(value(&row, "temp{k10temp-pci-00c3/Tctl}"), 45.875);
        assert_eq!(value(&row, "temp{zone/acpitz}"), 27.8);
        assert_eq!(value(&row, "btrfs.missing{data}"), 1.0);
        assert_eq!(value(&row, "btrfs.behind{data}"), 0.0);
        // Memory the kernel did not say is missing, not none.
        let row = series(None, &Snapshot::default());
        assert!(value(&row, "memory.used").is_nan());
    }

    #[test]
    fn a_machine_is_read_from_its_files() {
        let dir = folder("machine");
        let (processes, kernel, services) =
            (dir.join("processes"), dir.join("kernel"), dir.join("services"));
        put(
            &processes,
            "stat",
            "cpu0: user_ns=10 system_ns=20 idle_ns=70 switches=4\ncpus: 1\nuser_ns: 10\nsystem_ns: 20\nidle_ns: 70\nboot_time_ns: 1763000000000000000\n",
        );
        put(
            &processes,
            "meminfo",
            "total: 4096 KiB\nfree: 1024 KiB\npage_cache: 512 KiB\nswap_total: 0 KiB\nswap_free: 0 KiB\n",
        );
        put(&processes, "loadavg", "load_1: 0.50\nload_5: 0.40\nload_15: 0.30\n");
        put(&processes, "mounts", &format!("vda1 {} ext4 rw\n", dir.display()));
        put(
            &processes,
            "1/status",
            "number: 1\nname: spark\ndomain: system\nuser_ns: 5\nsystem_ns: 5\nresident_memory: 100 KiB\n",
        );
        put(
            &processes,
            "20/status",
            "number: 20\nname: web\ndomain: web\nuser_ns: 300\nsystem_ns: 100\nresident_memory: 2048 KiB\n",
        );
        put(&processes, "stat.old", "not a process");
        put(&services, "web.toml", "service_version = 1\n");
        put(&services, "settings.d/web.toml", "override\n");
        put(&kernel, "block/vda/stat", "1 0 2 3 4 0 5 6 0 7 8\n");
        let mut reader = Reader::new(Sources { processes, kernel, services });
        let snapshot = reader.read();
        assert_eq!(snapshot.cpus.len(), 1);
        assert_eq!((snapshot.cpu.user_ns, snapshot.cpu.idle_ns), (10, 70));
        assert_eq!(snapshot.boot_time_ns, 1_763_000_000_000_000_000);
        assert_eq!(snapshot.load, [0.5, 0.4, 0.3]);
        assert_eq!((snapshot.memory.total, snapshot.memory.available()), (4096, 1536));
        assert_eq!(snapshot.disks.len(), 1);
        assert_eq!(snapshot.filesystems.len(), 1);
        assert_eq!(
            snapshot.services,
            [Service { name: "web".into(), cpu_ns: 400, memory_bytes: 2048 * 1024 }]
        );
        assert!(snapshot.monotonic_ns > 0 && snapshot.at > 1_700_000_000);
        // A machine with none of it is read as one with nothing in it.
        let nothing = Reader::new(Sources {
            processes: dir.join("no"),
            kernel: dir.join("no"),
            services: dir.join("no"),
        })
        .read();
        assert!(
            nothing.disks.is_empty() && nothing.services.is_empty() && nothing.memory.total == 0
        );
    }
}
