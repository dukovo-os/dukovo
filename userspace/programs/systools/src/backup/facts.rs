// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a round says of the machine in its record (ADR 0464, format 2):
//! the hardware a person moving to new hardware compares, the texts of
//! the network and firewall settings, and the names of people's accounts.
//! They are read from the kernel's `/kernel` files and the settings
//! files, as `network show`, `serial` and `admin about` read them, so
//! that each of those commands and the record agree.
//!
//! The disks are those of the root's Btrfs, the volume a restore of the
//! whole machine replaces, and not every disk in the machine: a backup
//! disk plugged in for the round is none of the machine's. The root is a
//! mirror when the kernel's profiles of its data and its metadata all keep
//! a copy on each of two disks or more, as `storage show` reads them.
//!
//! Where it misbehaves, `backup show` and the record's text, which
//! `backup restore --machine --plan` prints in part, say what a round
//! wrote.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::record::{
    ACCOUNTS_MOST, CARDS_MOST, Card, DISKS_MOST, Disk, Hardware, SETTINGS_MOST, Serial, Settings,
    TEXT_MOST,
};
use crate::dhcp_server::mac_of;
use crate::ip::{self, Kind};
use crate::serial::{self, TTY_CLASS};
use crate::serial_console::{self, Choice};
use crate::util::{self, Fail};
use crate::{firewall, netd, probe};

/// Where the machine says what it has, so that the tests can read their
/// own.
pub struct Sources {
    /// The kernel's network cards.
    pub net: PathBuf,
    /// The kernel's disks.
    pub block: PathBuf,
    /// The kernel's Btrfs volumes, a folder for each, named by its UUID.
    pub btrfs: PathBuf,
    /// The kernel's serial ports.
    pub tty: PathBuf,
    /// The kernel's command line, which names the serial console.
    pub cmdline: PathBuf,
    /// network(5)'s file and firewall(8)'s.
    pub network: PathBuf,
    pub firewall: PathBuf,
    /// The accounts file.
    pub passwd: PathBuf,
}

impl Sources {
    /// The files of a running machine.
    pub fn system() -> Sources {
        Sources {
            net: PathBuf::from(ip::INTERFACES),
            block: PathBuf::from(probe::BLOCKS),
            btrfs: PathBuf::from(crate::storage::BTRFS),
            tty: PathBuf::from(TTY_CLASS),
            cmdline: PathBuf::from("/kernel/cmdline"),
            network: PathBuf::from(netd::SETTINGS),
            firewall: PathBuf::from(firewall::SETTINGS),
            passwd: PathBuf::from(accounts::PASSWD),
        }
    }
}

/// `text` as a record carries it: plain, without line ends, and no longer
/// than [`TEXT_MOST`] bytes. None when nothing is left of it.
fn tidy(text: &str) -> Option<String> {
    let plain: String =
        text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>();
    let plain = plain.trim();
    let mut end = plain.len().min(TEXT_MOST);
    while !plain.is_char_boundary(end) {
        end -= 1;
    }
    (end > 0).then(|| plain[..end].trim_end().to_string())
}

/// What the file `name` of folder `dir` says, trimmed.
fn read(dir: &Path, name: &str) -> Option<String> {
    fs::read_to_string(dir.join(name)).ok().map(|text| text.trim().to_string())
}

/// The model of a card, from the `model` line of its `card` file, which
/// the kernel writes for a card it drives.
fn card_model(dir: &Path) -> Option<String> {
    let card = read(dir, "card")?;
    card.lines().find_map(|line| line.strip_prefix("model ")).and_then(tidy)
}

/// The network cards: the kernel's cards with a MAC address of their own,
/// in the order of their numbers. A bridge, a VLAN, a tunnel and the
/// loopback are not cards.
fn cards(net: &Path) -> Vec<Card> {
    let Ok(interfaces) = ip::read_interfaces(net, Path::new(ip::ROUTES)) else {
        return Vec::new();
    };
    interfaces
        .into_iter()
        .filter(|card| card.kind == Kind::Card && !card.loopback)
        .filter(|card| mac_of(&card.mac).is_some_and(|mac| mac != [0; 6]))
        .filter_map(|card| {
            let model = card_model(&net.join(&card.name));
            Some(Card { name: tidy(&card.name)?, mac: card.mac.to_ascii_lowercase(), model })
        })
        .take(CARDS_MOST)
        .collect()
}

/// The disks that the partitions `partitions` are on, each once, in the
/// order of the partitions: its model if the driver gives one, and its
/// size in bytes. A disk the kernel no longer lists is left out.
fn disks(block: &Path, partitions: &[String]) -> Vec<Disk> {
    let mut named: Vec<&str> = Vec::new();
    for partition in partitions {
        let disk = probe::disk_of(partition);
        if !named.contains(&disk) {
            named.push(disk);
        }
    }
    named
        .into_iter()
        .filter_map(|disk| {
            let dir = block.join(disk);
            let sectors: u64 = read(&dir, "size")?.parse().ok()?;
            let model = read(&dir.join("device"), "model").as_deref().and_then(tidy);
            Some(Disk { model, size: sectors.saturating_mul(512) })
        })
        .take(DISKS_MOST)
        .collect()
}

/// The profiles that keep a copy of each block on two disks or more.
const MIRRORS: [&str; 4] = ["raid1", "raid1c3", "raid1c4", "raid10"];

/// Whether the volume whose `profiles` file says `text` mirrors its data
/// and its metadata: each of the two kinds has block groups, and every
/// profile of them, two while a balance converts it, is one of
/// [`MIRRORS`].
fn mirrored(text: &str) -> bool {
    ["data", "metadata"].iter().all(|kind| {
        let profiles: Vec<&str> = text
            .lines()
            .filter_map(|line| line.split_once(' '))
            .filter(|(named, _)| named == kind)
            .flat_map(|(_, rest)| rest.split_whitespace())
            .collect();
        !profiles.is_empty() && profiles.iter().all(|profile| MIRRORS.contains(profile))
    })
}

/// The serial port the machine's console is on, as serial-console(8)
/// chooses it from the kernel's flags: the last `console=ttyS` flag, or
/// the port the firmware sends its console to. None when it keeps a
/// console on no port, or on every port.
fn serial_console(src: &Sources) -> Option<Serial> {
    let line = fs::read_to_string(&src.cmdline).unwrap_or_default();
    let found = serial::ports(&src.tty);
    let ports: Vec<(String, Option<String>)> =
        found.iter().map(|port| (port.name.clone(), port.firmware.clone())).collect();
    let device = match serial_console::choose(&line, &ports) {
        Choice::Flags(mut asked) => asked.pop()?.name,
        Choice::Firmware(asked) => asked.name,
        Choice::Any(_) | Choice::Nothing => return None,
    };
    let model = found.iter().find(|port| port.name == device).and_then(|port| tidy(&port.kind));
    Some(Serial { device: tidy(&device)?, model })
}

/// The hardware of the machine whose root is the Btrfs `uuid` on
/// `partitions`, with processor `cpu`. A root whose profiles the kernel
/// does not give is no mirror.
pub fn hardware(src: &Sources, uuid: &str, partitions: &[String], cpu: Option<&str>) -> Hardware {
    let profiles = (!uuid.is_empty() && !uuid.contains('/'))
        .then(|| fs::read_to_string(src.btrfs.join(uuid).join("profiles")).ok())
        .flatten();
    Hardware {
        cards: cards(&src.net),
        disks: disks(&src.block, partitions),
        mirror: profiles.as_deref().is_some_and(mirrored),
        serial: serial_console(src),
        cpu: cpu.and_then(tidy),
    }
}

/// The text of the settings file `path`: empty when there is none, and
/// None when it is larger than a record carries or is not text.
fn settings_text(path: &Path) -> Result<Option<String>, Fail> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Some(String::new())),
        Err(err) => return Err(util::failed(&path.display(), err)),
    };
    let mut bytes = Vec::new();
    file.take(SETTINGS_MOST as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| util::failed(&path.display(), err))?;
    if bytes.len() > SETTINGS_MOST {
        return Ok(None);
    }
    Ok(String::from_utf8(bytes).ok())
}

/// The settings a record carries: network(5)'s file and firewall(8)'s.
///
/// # Errors
///
/// A settings file that is there and cannot be read.
pub fn settings(src: &Sources) -> Result<Settings, Fail> {
    Ok(Settings { network: settings_text(&src.network)?, firewall: settings_text(&src.firewall)? })
}

/// The names of people's accounts in the accounts file `passwd`, in its
/// order; none when there is no file.
///
/// # Errors
///
/// An accounts file that is there and cannot be read.
pub fn accounts(passwd: &Path) -> Result<Vec<String>, Fail> {
    let text = match fs::read_to_string(passwd) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(util::failed(&passwd.display(), err)),
    };
    Ok(accounts::users(&text)
        .entries()
        .filter(|user| user.is_person())
        .map(|user| user.name.clone())
        .take(ACCOUNTS_MOST)
        .collect())
}

/// The hardware of this machine, as it is now.
pub fn this_machine() -> Hardware {
    let (uuid, partitions) =
        crate::sysupdate::place::root().map_or_default(|root| (root.uuid, root.partitions));
    let cpu = crate::lscpu::model_name(&crate::lscpu::cpuid)
        .map(|brand| crate::dukofetch::cpu_name(&brand));
    hardware(&Sources::system(), &uuid, &partitions, cpu.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine's `/kernel` and settings files, in a folder of its own.
    struct Machine {
        root: PathBuf,
    }

    impl Machine {
        fn new(name: &str) -> Machine {
            let root =
                std::env::temp_dir().join(format!("backup-facts-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Machine { root }
        }

        fn put(&self, path: &str, text: &str) {
            let path = self.root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        /// A card with the files `ip` reads, and the `card` file the
        /// kernel writes for one it drives.
        fn card(&self, name: &str, index: u32, mac: &str, link: Option<&str>, model: Option<&str>) {
            let base = format!("net/{name}");
            self.put(&format!("{base}/ifindex"), &format!("{index}\n"));
            self.put(&format!("{base}/address"), &format!("{mac}\n"));
            self.put(&format!("{base}/type"), "1\n");
            if let Some(link) = link {
                self.put(&format!("{base}/link"), link);
            }
            if let Some(model) = model {
                self.put(
                    &format!("{base}/card"),
                    &format!("driver x\nmodel {model}\nbus 0000:00:03.0\n"),
                );
            }
        }

        fn disk(&self, name: &str, sectors: u64, model: Option<&str>) {
            self.put(&format!("block/{name}/size"), &format!("{sectors}\n"));
            if let Some(model) = model {
                self.put(&format!("block/{name}/device/model"), &format!("{model}\n"));
            }
        }

        fn sources(&self) -> Sources {
            Sources {
                net: self.root.join("net"),
                block: self.root.join("block"),
                btrfs: self.root.join("btrfs"),
                tty: self.root.join("tty"),
                cmdline: self.root.join("cmdline"),
                network: self.root.join("network"),
                firewall: self.root.join("firewall"),
                passwd: self.root.join("passwd"),
            }
        }
    }

    impl Drop for Machine {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn partitions(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn the_cards_are_the_kernels_with_a_mac_address_of_their_own() {
        let machine = Machine::new("cards");
        machine.card("net1", 3, "52:54:00:12:34:57", None, None);
        machine.card("net0", 2, "52:54:00:12:34:56", None, Some("Intel 82540EM"));
        machine.card("lo", 1, "00:00:00:00:00:00", Some("loopback\n"), None);
        machine.card("br0", 4, "52:54:00:aa:aa:aa", Some("bridge\nport net1\n"), None);
        machine.card("wg0", 5, "00:00:00:00:00:00", Some("tunnel\n"), None);
        machine.card("net2", 6, "00:00:00:00:00:00", None, None);
        assert_eq!(
            cards(&machine.sources().net),
            [
                Card {
                    name: "net0".into(),
                    mac: "52:54:00:12:34:56".into(),
                    model: Some("Intel 82540EM".into())
                },
                Card { name: "net1".into(), mac: "52:54:00:12:34:57".into(), model: None },
            ]
        );
        assert_eq!(cards(&machine.root.join("none")), []);
    }

    #[test]
    fn the_disks_are_those_the_root_is_on_each_once() {
        let machine = Machine::new("disks");
        machine.disk("sda", 3_907_029_168, Some("WD20EFZX"));
        machine.disk("sdb", 3_907_029_168, Some("WD20EFZX"));
        machine.disk("sdc", 1000, Some("A backup disk"));
        machine.disk("nvme0n1", 1_000_000, None);
        let block = machine.sources().block;
        let one = disks(&block, &partitions(&["sda2", "sda3"]));
        assert_eq!(one, [Disk { model: Some("WD20EFZX".into()), size: 2_000_398_934_016 }]);
        let two = disks(&block, &partitions(&["sda3", "sdb3"]));
        assert_eq!(two.len(), 2);
        let nvme = disks(&block, &partitions(&["nvme0n1p3"]));
        assert_eq!(nvme, [Disk { model: None, size: 512_000_000 }]);
        // A disk the kernel does not list is not in the record.
        assert_eq!(disks(&block, &partitions(&["sdz1"])), []);
        assert_eq!(disks(&block, &[]), []);
    }

    #[test]
    fn a_root_is_a_mirror_when_its_data_and_metadata_are_kept_twice() {
        let machine = Machine::new("mirror");
        machine.disk("sda", 100, None);
        machine.disk("sdb", 100, None);
        let src = machine.sources();
        let two = partitions(&["sda3", "sdb3"]);
        let with = |profiles: &str| {
            machine.put("btrfs/u-1/profiles", profiles);
            hardware(&src, "u-1", &two, None).mirror
        };
        assert!(with("data raid1\nmetadata raid1\nsystem raid1\n"));
        assert!(with("data raid10\nmetadata raid1c3\nsystem raid1c3\n"));
        // Two disks whose data is on one or the other are no mirror.
        assert!(!with("data single\nmetadata raid1\nsystem raid1\n"));
        // While a balance converts the data, part of it has one copy.
        assert!(!with("data single raid1\nmetadata raid1\nsystem raid1\n"));
        assert!(!with("data raid1\nmetadata dup\nsystem dup\n"));
        assert!(!with("data raid1\n"));
        assert!(!with(""));
        // Without the kernel's profiles, or the root's UUID, no mirror is claimed.
        assert!(!hardware(&src, "u-2", &two, None).mirror);
        assert!(!hardware(&src, "", &two, None).mirror);
        assert!(!hardware(&src, "../u-1", &two, None).mirror);
        assert_eq!(hardware(&src, "u-1", &two, None).disks.len(), 2);
    }

    #[test]
    fn the_serial_console_is_the_port_the_kernels_last_flag_names() {
        let machine = Machine::new("serial");
        for (name, kind) in [("ttyS0", "com1"), ("ttyS1", "com2"), ("ttyS4", "amt-sol")] {
            machine.put(&format!("tty/{name}/port"), "0x2F8\n");
            machine.put(&format!("tty/{name}/console"), "N\n");
            machine.put(&format!("tty/{name}/kind"), &format!("{kind}\n"));
        }
        let src = machine.sources();
        let with = |line: &str| {
            machine.put("cmdline", line);
            serial_console(&src)
        };
        assert_eq!(
            with("root=x console=ttyS1,57600 quiet\n"),
            Some(Serial { device: "ttyS1".into(), model: Some("com2".into()) })
        );
        assert_eq!(
            with("console=ttyS1 console=ttyS4,115200n8\n"),
            Some(Serial { device: "ttyS4".into(), model: Some("amt-sol".into()) })
        );
        // A screen's flag after the port's makes no difference to it.
        assert_eq!(with("console=ttyS0 console=tty0\n").unwrap().device, "ttyS0");
        assert_eq!(with("root=x quiet\n"), None);
        assert_eq!(with("serial-console=any\n"), None);
    }

    #[test]
    fn the_port_the_firmware_names_is_the_console_when_no_flag_says_another() {
        let machine = Machine::new("firmware");
        machine.put("tty/ttyS1/port", "0x2F8\n");
        machine.put("tty/ttyS1/console", "N\n");
        machine.put("tty/ttyS1/kind", "com2\n");
        machine.put("tty/ttyS1/firmware_console", "57600n8\n");
        machine.put("cmdline", "root=x\n");
        let found = serial_console(&machine.sources()).unwrap();
        assert_eq!((found.device.as_str(), found.model.as_deref()), ("ttyS1", Some("com2")));
        machine.put("cmdline", "root=x spcr=off\n");
        assert_eq!(serial_console(&machine.sources()), None);
    }

    #[test]
    fn a_name_in_a_record_is_plain_and_short() {
        assert_eq!(tidy("  Intel Core i5-6500  ").as_deref(), Some("Intel Core i5-6500"));
        assert_eq!(tidy("a\u{1b}[31mb\n").as_deref(), Some("a [31mb"));
        assert_eq!(tidy("   "), None);
        assert_eq!(tidy(""), None);
        let long = "\u{e9}".repeat(TEXT_MOST);
        let cut = tidy(&long).unwrap();
        assert!(cut.len() <= TEXT_MOST && long.starts_with(&cut), "{}", cut.len());
    }

    #[test]
    fn the_settings_are_the_files_texts_empty_without_one_and_none_when_too_large() {
        let machine = Machine::new("settings");
        let src = machine.sources();
        assert_eq!(
            settings(&src).unwrap(),
            Settings { network: Some(String::new()), firewall: Some(String::new()) }
        );
        machine.put("network", "[net0]\naddress 192.168.1.12/24\n");
        machine.put("firewall", &"a".repeat(SETTINGS_MOST));
        let found = settings(&src).unwrap();
        assert_eq!(found.network.as_deref(), Some("[net0]\naddress 192.168.1.12/24\n"));
        assert_eq!(found.firewall.map(|text| text.len()), Some(SETTINGS_MOST));
        machine.put("firewall", &"a".repeat(SETTINGS_MOST + 1));
        assert_eq!(settings(&src).unwrap().firewall, None);
        fs::write(&src.network, [0xff, 0xfe, 0x41]).unwrap();
        assert_eq!(settings(&src).unwrap().network, None);
    }

    #[test]
    fn the_accounts_are_the_people_of_the_accounts_file_in_its_order() {
        let machine = Machine::new("accounts");
        let src = machine.sources();
        assert_eq!(accounts(&src.passwd).unwrap(), Vec::<String>::new());
        machine.put(
            "passwd",
            "root:x:0:0:root:/system/root:/system/commands/nukesh\n\
             owner:x:1000:1000:The Owner:/users/owner:/system/commands/nukesh\n\
             postgres:x:120:120:PostgreSQL:/applications/x:/system/commands/false\n\
             not a line\n\
             helper:x:1001:1001::/users/helper:/system/commands/nukesh\n",
        );
        assert_eq!(accounts(&src.passwd).unwrap(), ["owner", "helper"]);
        fs::remove_file(&src.passwd).unwrap();
        fs::create_dir_all(&src.passwd).unwrap();
        assert!(accounts(&src.passwd).is_err());
    }

    #[test]
    fn a_settings_file_that_cannot_be_read_is_an_error_and_not_an_empty_text() {
        let machine = Machine::new("unreadable");
        let mut src = machine.sources();
        // A folder where the file should be is a file that is there and
        // cannot be read as one.
        fs::create_dir_all(&src.network).unwrap();
        assert!(settings(&src).is_err());
        src.network = machine.root.join("network-not-there");
        assert!(settings(&src).is_ok());
    }
}
