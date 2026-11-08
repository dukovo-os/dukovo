// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the computer has, for the installer's screens to offer: its disks
//! and what is on them, its network cards, its memory, and the keyboard
//! layouts and time zones Dukovo knows. Everything is read from the
//! kernel's files (radix-sysfs(5), radix-proc(5)) and the system's
//! resources when the screens start; the tests make a [`Machine`] by hand.

use std::fs;
use std::path::Path;

use crate::disks;
use crate::windows::{self, Windows};

/// Where the kernel lists disks and partitions.
const BLOCK: &str = "/kernel/block";

/// Where the kernel lists network interfaces.
const NET: &str = "/kernel/class/net";

/// The tz database's list of time zones, one a country or region.
const ZONES: &str = "/system/resources/zoneinfo/zone1970.tab";

/// The kernel's memory counts.
const MEMINFO: &str = "/processes/meminfo";

/// A disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskInfo {
    /// Its name in `/devices`: `vda`, `nvme0n1`.
    pub name: String,
    /// Its size in bytes.
    pub bytes: u64,
    /// Its model, as it gives it, or nothing.
    pub model: String,
    /// Whether it spins: `Some(false)` for an SSD.
    pub rotational: Option<bool>,
    /// The kinds of its partitions, as `/kernel/block/<name>/parttype`
    /// gives them: a GPT type's GUID in lowercase or an MBR byte.
    pub partitions: Vec<String>,
    /// Whether it is the install stick this system runs from.
    pub stick: bool,
    /// The size of its logical sectors, in bytes: 512 or 4096.
    pub sector_size: u32,
    /// Its partition table, as rdisk(8) reads it, if it has one: what the
    /// free-space layout and the rdisk screen work from.
    pub table: Option<disk::Table>,
    /// Windows on it, if it is there, for the layout next to it.
    pub windows: Option<Windows>,
}

/// What a disk holds, as far as its partitions' kinds tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holds {
    /// No partitions.
    Nothing,
    /// Windows: a Microsoft reserved, basic data or recovery partition.
    Windows,
    /// A Dukovo root.
    Dukovo,
    /// A Linux filesystem.
    Linux,
    /// Partitions of other kinds.
    Other,
}

/// GPT types that mean Windows is there: Microsoft reserved, basic data
/// and Windows recovery, and NTFS's MBR byte.
const WINDOWS_KINDS: [&str; 4] = [
    "e3c9e316-0b5c-4db8-817d-f92df00215ae",
    "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7",
    "de94bba4-06d1-4d40-a16a-bfd50179d6ac",
    "0x07",
];

/// The GPT type of a Dukovo root (ADR 0137).
const DUKOVO_ROOT: &str = "90c9a303-3954-4174-8f8b-ee75a92a9eb0";

/// GPT types of Linux filesystems, root and home, and Linux's MBR byte.
const LINUX_KINDS: [&str; 4] = [
    "0fc63daf-8483-4772-8e79-3d69d8477de4",
    "4f68bce3-e8cd-4db1-96e7-fbcaf984b709",
    "933ac7e1-2eb4-4f13-b844-0e14e2aef915",
    "0x83",
];

impl DiskInfo {
    /// How many logical sectors it has.
    #[must_use]
    pub fn sectors(&self) -> u64 {
        self.bytes / u64::from(self.sector_size.max(1))
    }

    /// What it holds. Windows is named first: a disk with Windows and a
    /// Linux partition is still the disk Windows starts from.
    #[must_use]
    pub fn holds(&self) -> Holds {
        let has =
            |kinds: &[&str]| self.partitions.iter().any(|kind| kinds.contains(&kind.as_str()));
        if self.partitions.is_empty() {
            Holds::Nothing
        } else if has(&WINDOWS_KINDS) {
            Holds::Windows
        } else if has(&[DUKOVO_ROOT]) {
            Holds::Dukovo
        } else if has(&LINUX_KINDS) {
            Holds::Linux
        } else {
            Holds::Other
        }
    }
}

/// A network card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// Its name: `net0`.
    pub name: String,
    /// Its model, as its driver gives it, or nothing.
    pub model: String,
    /// Whether a cable is in and the link is up.
    pub carrier: bool,
    /// The speed it negotiated, in megabits a second, if it says.
    pub speed: Option<u32>,
    /// Its IPv4 addresses with their prefixes, as DHCP or the live
    /// system's settings gave them.
    pub addresses: Vec<String>,
}

/// What the computer has.
#[derive(Clone, Debug, Default)]
pub struct Machine {
    /// Its disks, by name, the install stick among them.
    pub disks: Vec<DiskInfo>,
    /// Its network cards, by name.
    pub cards: Vec<Card>,
    /// Its memory, in bytes.
    pub memory: u64,
    /// The keyboard layouts, as (name, what it is called).
    pub layouts: Vec<(String, String)>,
    /// The time zones, by name, sorted, `UTC` first.
    pub zones: Vec<String>,
    /// Whether the firmware's boot entries can be made: the computer
    /// started by UEFI and the kernel reaches its variables, as efiboot(8)
    /// lists them. Installing next to Windows needs one.
    pub boot_entries: bool,
    /// Whether the installer runs on the first serial port, as the
    /// install stick's "Install Dukovo (serial console)" entry starts it: the
    /// installed system then keeps a console there (ADR 0318).
    pub serial_console: bool,
}

impl Machine {
    /// Reads what this computer has.
    #[must_use]
    pub fn probe() -> Machine {
        let stick = disks::live_partition().ok().map(|(name, _)| disks::disk_of(&name));
        let mut disks = read_disks(Path::new(BLOCK), stick.as_deref());
        for disk in &mut disks {
            // A disk whose table cannot be read is offered as an empty one,
            // which only the guided layout, erasing it, can use.
            disk.table = disks::read_table(&disk.name, disk.bytes, disk.sector_size).ok().flatten();
            if disk.holds() == Holds::Windows && !disk.stick {
                disk.windows = windows::find(disk);
            }
        }
        Machine {
            disks,
            cards: read_cards(Path::new(NET)),
            memory: read_memory(Path::new(MEMINFO)),
            layouts: layouts(),
            zones: read_zones(Path::new(ZONES)),
            boot_entries: boot_entries(),
            serial_console: on_serial_console(),
        }
    }
}

/// The first serial port, as a terminal: where serial-console(8) runs the
/// installer under the kernel flag `console=ttyS0`.
pub const SERIAL_LINE: &str = "/devices/ttyS0";

/// Whether the installer's terminal is the first serial port.
#[must_use]
pub fn on_serial_console() -> bool {
    // SAFETY: ttyname returns a C string the C library keeps, or null;
    // the installer asks before it starts any thread that would.
    let name = unsafe { libc::ttyname(0) };
    // SAFETY: a non-null result is a C string, which is read at once.
    !name.is_null()
        && unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes() == SERIAL_LINE.as_bytes()
}

/// The kernel flag that keeps a console on the first serial port, for the
/// installed system's menu: the live system's own `console=ttyS0` word,
/// with its speed if it gives one, or plain `console=ttyS0`.
#[must_use]
pub fn serial_console_flag() -> String {
    let line = fs::read_to_string("/kernel/cmdline").unwrap_or_default();
    console_word(&line).unwrap_or("console=ttyS0").to_string()
}

/// The last `console=ttyS0` word, with or without options, among the
/// kernel flags of command line `line`, those before a lone `--`.
fn console_word(line: &str) -> Option<&str> {
    line.split_whitespace()
        .take_while(|word| *word != "--")
        .filter(|word| {
            word.strip_prefix("console=ttyS0").is_some_and(|r| r.is_empty() || r.starts_with(','))
        })
        .last()
}

/// Whether efiboot(8) lists the firmware's boot entries: it fails when
/// the computer started by a BIOS, or the firmware's variables are out of
/// the kernel's reach.
#[must_use]
pub fn boot_entries() -> bool {
    std::process::Command::new("/system/commands/efiboot")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// What the kernel says of `name` in its file `attribute` under `block`,
/// without the newline.
fn attribute(block: &Path, name: &str, attribute: &str) -> Option<String> {
    let text = fs::read_to_string(block.join(name).join(attribute)).ok()?;
    Some(text.trim().to_string())
}

/// The whole disk `name` as the kernel lists it now, with its table: the
/// disk an install changes, read again as it starts.
///
/// # Errors
///
/// If there is no such disk, it is a partition, or its table is damaged.
pub fn read_disk(name: &str) -> Result<DiskInfo, String> {
    if disks::is_partition(name) {
        return Err(format!("{name} is a partition; name its disk, {}", disks::disk_of(name)));
    }
    let stick = disks::live_partition().ok().map(|(part, _)| disks::disk_of(&part));
    let mut disk = read_disks(Path::new(BLOCK), stick.as_deref())
        .into_iter()
        .find(|disk| disk.name == name)
        .ok_or_else(|| format!("there is no disk {name}: lsblk lists them"))?;
    disk.table = disks::read_table(&disk.name, disk.bytes, disk.sector_size)?;
    if disk.holds() == Holds::Windows {
        disk.windows = windows::find(&disk);
    }
    Ok(disk)
}

/// The whole disks in `block`, each with its partitions' kinds; the disk
/// named `stick` is marked as the install stick. Their tables are read
/// apart, from the disks themselves.
fn read_disks(block: &Path, stick: Option<&str>) -> Vec<DiskInfo> {
    let mut names: Vec<String> = fs::read_dir(block)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    let (partitions, whole): (Vec<&String>, Vec<&String>) =
        names.iter().partition(|name| attribute(block, name, "partition").is_some());
    whole
        .into_iter()
        .map(|name| {
            let sectors = attribute(block, name, "size").and_then(|s| s.parse::<u64>().ok());
            DiskInfo {
                name: name.clone(),
                bytes: sectors.unwrap_or(0) * 512,
                model: attribute(block, name, "device/model").unwrap_or_default(),
                rotational: attribute(block, name, "queue/rotational").map(|r| r == "1"),
                partitions: partitions
                    .iter()
                    .filter(|part| disks::disk_of(part) == *name)
                    .filter_map(|part| attribute(block, part, "parttype"))
                    .collect(),
                stick: stick == Some(name.as_str()),
                sector_size: attribute(block, name, "queue/logical_block_size")
                    .and_then(|size| size.parse().ok())
                    .unwrap_or(512),
                table: None,
                windows: None,
            }
        })
        // A disk of no size is a card reader with no card in it.
        .filter(|disk| disk.bytes > 0)
        .collect()
}

/// The network cards in `net`: the interfaces whose `link` file says
/// `card`, not the loopback or those made of other interfaces (bridges,
/// VLANs, tunnels).
fn read_cards(net: &Path) -> Vec<Card> {
    let mut cards: Vec<Card> = fs::read_dir(net)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let read = |file: &str| fs::read_to_string(entry.path().join(file)).ok();
            let link = read("link").unwrap_or_default();
            if link.lines().next() != Some("card") {
                return None;
            }
            let model = read("card")
                .unwrap_or_default()
                .lines()
                .find_map(|line| line.strip_prefix("model ").map(str::to_string))
                .unwrap_or_default();
            Some(Card {
                name,
                model,
                carrier: read("carrier").is_some_and(|c| c.trim() == "1"),
                speed: read("speed").and_then(|s| s.trim().parse().ok()),
                addresses: read("ipv4")
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|line| line.split_whitespace().next().map(str::to_string))
                    .collect(),
            })
        })
        .collect();
    cards.sort_by(|a, b| a.name.cmp(&b.name));
    cards
}

/// The memory the kernel manages, in bytes: `total` in `meminfo`, in KiB.
fn read_memory(meminfo: &Path) -> u64 {
    let text = fs::read_to_string(meminfo).unwrap_or_default();
    text.lines()
        .find_map(|line| {
            let mut words = line.split_whitespace();
            (words.next()?.trim_end_matches(':') == "total")
                .then(|| words.next()?.parse::<u64>().ok())
                .flatten()
        })
        .map_or(0, |kib| kib * 1024)
}

/// The time zones in the tz database's `zone1970.tab`, the third column
/// of each line, sorted, with `UTC` first.
fn read_zones(table: &Path) -> Vec<String> {
    let text = fs::read_to_string(table).unwrap_or_default();
    let mut zones: Vec<String> = text
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split('\t').nth(2).map(str::to_string))
        .collect();
    zones.sort();
    zones.dedup();
    zones.insert(0, "UTC".into());
    zones
}

/// Every built-in keyboard layout, as keymap(1) lists them.
fn layouts() -> Vec<(String, String)> {
    keymap::LAYOUTS
        .iter()
        .map(|(name, _)| {
            let shown = keymap::Keymap::builtin(name).map_or_else(|| name.to_string(), |k| k.name);
            (name.to_string(), shown)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_serial_consoles_flag_is_the_live_systems_own() {
        assert_eq!(
            console_word("live install console=ttyS0 console-sign-in=off"),
            Some("console=ttyS0")
        );
        assert_eq!(
            console_word("console=ttyS0 console=ttyS0,9600n8 quiet"),
            Some("console=ttyS0,9600n8")
        );
        assert_eq!(console_word("console=tty0 console=ttyS1"), None);
        assert_eq!(console_word("live -- console=ttyS0"), None);
    }

    fn write(dir: &Path, file: &str, text: &str) {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dukovo-install-machine-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn disks_are_read_with_their_partitions() {
        let dir = scratch("disks");
        write(&dir, "sda/size", "2097152\n");
        write(&dir, "sda/device/model", "SK hynix SC311\n");
        write(&dir, "sda/queue/rotational", "0\n");
        write(&dir, "sda/queue/logical_block_size", "4096\n");
        write(&dir, "sda1/partition", "1\n");
        write(&dir, "sda1/parttype", "c12a7328-f81f-11d2-ba4b-00a0c93ec93b\n");
        write(&dir, "sda2/partition", "2\n");
        write(&dir, "sda2/parttype", "e3c9e316-0b5c-4db8-817d-f92df00215ae\n");
        write(&dir, "sdb/size", "1048576\n");
        write(&dir, "sdb1/partition", "1\n");
        write(&dir, "sdb1/parttype", "5894c662-af90-4b47-baaf-2e3396443b25\n");
        write(&dir, "vda/size", "8388608\n");
        write(&dir, "sr0/size", "0\n");
        let disks = read_disks(&dir, Some("sdb"));
        let names: Vec<&str> = disks.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["sda", "sdb", "vda"]);
        assert_eq!(disks[0].bytes, 1 << 30);
        assert_eq!(disks[0].model, "SK hynix SC311");
        assert_eq!(disks[0].rotational, Some(false));
        assert_eq!((disks[0].sector_size, disks[0].sectors()), (4096, 1 << 18));
        assert_eq!(disks[2].sector_size, 512);
        assert_eq!(disks[0].holds(), Holds::Windows);
        assert!(disks[1].stick && !disks[0].stick);
        assert_eq!(disks[1].holds(), Holds::Other);
        assert_eq!(disks[2].holds(), Holds::Nothing);
        assert_eq!(disks[2].rotational, None);
    }

    #[test]
    fn cards_leave_out_the_loopback_and_bridges() {
        let dir = scratch("cards");
        write(&dir, "lo/link", "loopback\n");
        write(&dir, "br0/type", "1\n");
        write(&dir, "br0/link", "bridge\n");
        write(&dir, "net0/link", "card\n");
        write(&dir, "net0/carrier", "1\n");
        write(&dir, "net0/speed", "1000\n");
        write(&dir, "net0/card", "driver radix-intel-net\nmodel Intel 82574L\n");
        write(&dir, "net0/ipv4", "10.0.2.15/24 dhcp\n");
        let cards = read_cards(&dir);
        assert_eq!(
            cards,
            [Card {
                name: "net0".into(),
                model: "Intel 82574L".into(),
                carrier: true,
                speed: Some(1000),
                addresses: vec!["10.0.2.15/24".into()],
            }]
        );
    }

    #[test]
    fn memory_and_zones_are_read() {
        let dir = scratch("misc");
        write(&dir, "meminfo", "total: 1048576 KiB\nfree: 4 KiB\n");
        assert_eq!(read_memory(&dir.join("meminfo")), 1 << 30);
        write(
            &dir,
            "zone1970.tab",
            "# comment\nUS\t+394421-1045903\tAmerica/Denver\tMountain\nAD\t+4230+00131\tEurope/Andorra\n",
        );
        assert_eq!(
            read_zones(&dir.join("zone1970.tab")),
            ["UTC", "America/Denver", "Europe/Andorra"]
        );
        assert!(layouts().iter().any(|(name, shown)| name == "us" && !shown.is_empty()));
    }
}
