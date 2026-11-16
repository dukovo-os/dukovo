// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `hardware devices` says: every PCI function and USB device the
//! kernel found, each with what it is to the system, the driver that took
//! it where the system can tell, its state and where it is.
//!
//! The kernel lists a PCI function whether or not a driver uses it and
//! says nothing of which does. What can be told is told: a network card
//! says its driver and its PCI address (radix-sysfs(5), `class/net/*/card`)
//! and whether its link is up; a USB controller is its root hubs' serial
//! number; a USB interface has a link to the driver that took it, or has
//! none. A SATA or NVMe controller's driver is told by its class, which the
//! kernel gives to its AHCI and NVMe drivers whole: every function of the
//! SATA class (01:06:01) is the AHCI driver's, every one of the NVMe class
//! (01:08:02) the NVMe driver's. For the rest the driver is not told, and
//! the row says so: not knowing a driver is not the same as having none.
//!
//! A disk is named on its controller only where the kernel's names leave no
//! doubt: the kernel names a SATA and a USB disk alike (`sda`), so the
//! `sd` disks are a SATA controller's only when it is the only one and no
//! USB interface is taken by a mass storage driver.
//!
//! Bridges and hubs work and need nothing, so they are `hidden`: the
//! page folds them.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::lspci::{self, Function, Ids, vendor_device};
use crate::lsusb::{self, Device as UsbDevice, RootHub};

/// Where the kernel lists the disks, the network interfaces and the PCI
/// functions and USB devices.
const BLOCKS: &str = "/kernel/block";
const NET: &str = "/kernel/class/net";

/// The SATA controller class, 01:06:01, and the NVMe one, 01:08:02, which
/// the kernel binds every function of to the AHCI and the NVMe driver
/// (`kernel/radix/src/fs/disk/mod.rs`, where the PCI functions are
/// scanned and bound).
const AHCI_CLASS: u32 = 0x01_06_01;
const NVME_CLASS: u32 = 0x01_08_02;

/// A PCI function's base class and subclass for a USB controller, and
/// the base classes of the rest.
const USB_CONTROLLER: (u8, u8) = (0x0c, 0x03);
const STORAGE: u8 = 0x01;
const NETWORK: u8 = 0x02;
const DISPLAY: u8 = 0x03;
const BRIDGE: u8 = 0x06;
/// A USB interface's class for a hub.
const HUB: u8 = 0x09;

/// Where each source is, so that tests can read their own.
pub(crate) struct Sources {
    pci: PathBuf,
    usb: PathBuf,
    net: PathBuf,
    blocks: PathBuf,
    pci_ids: PathBuf,
    usb_ids: PathBuf,
}

impl Sources {
    /// Where the system keeps them.
    pub(crate) fn system() -> Sources {
        Sources {
            pci: PathBuf::from(lspci::DEVICES),
            usb: PathBuf::from(lsusb::DEVICES),
            net: PathBuf::from(NET),
            blocks: PathBuf::from(BLOCKS),
            pci_ids: PathBuf::from(lspci::IDS),
            usb_ids: PathBuf::from(lsusb::IDS),
        }
    }
}

/// What `hardware devices --json` says.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Devices {
    /// The version of this document: 1.
    format: u32,
    devices: Vec<Device>,
}

/// The part of the page a device is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Group {
    Network,
    Storage,
    Usb,
    Display,
    Other,
}

/// How a device is doing, where that can be told.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
enum State {
    /// Its driver took it, and it is in use.
    #[serde(rename = "working")]
    Working,
    /// A network card whose link is down.
    #[serde(rename = "no-link")]
    NoLink,
    /// The system knows no driver took it: a USB interface with none.
    #[serde(rename = "no-driver")]
    NoDriver,
}

/// A PCI function or a USB device.
#[derive(Debug, PartialEq, Serialize)]
struct Device {
    group: Group,
    /// What it is called: the vendor's and the device's names, or their
    /// numbers when the databases do not know them.
    name: String,
    /// Its vendor and device numbers, `8086:1533`; the numbers a report
    /// may carry, never a serial.
    ids: String,
    /// What kind of thing it is, `Ethernet controller`; null where the
    /// database has no word.
    class: Option<String>,
    /// What it is to the system: `net1`, or `sda, sdb`.
    system: Vec<String>,
    /// The driver that took it; null where none is told of.
    driver: Option<String>,
    /// Whether the system says anything of its driver: false is "not
    /// told", true with a null driver is "none took it".
    driver_told: bool,
    state: Option<State>,
    /// Where it is: `PCI 0000:03:00.0`, `USB 1-3.2`.
    #[serde(rename = "where")]
    place: String,
    /// Whether the page folds it away: bridges and hubs.
    hidden: bool,
}

/// A network card, as `class/net/NAME/card` and `operstate` say it.
struct Card {
    name: String,
    driver: Option<String>,
    bus: Option<String>,
    link_down: bool,
}

/// The cards in `dir` that say they are on a bus.
fn cards(dir: &Path) -> Vec<Card> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<Card> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let text = fs::read_to_string(entry.path().join("card")).ok()?;
            let line = |key: &str| {
                text.lines().find_map(|line| {
                    let value = line.strip_prefix(key)?.strip_prefix(' ')?.trim();
                    (!value.is_empty()).then(|| value.to_string())
                })
            };
            let state = fs::read_to_string(entry.path().join("operstate")).ok();
            Some(Card {
                name,
                driver: line("driver"),
                bus: line("bus"),
                link_down: state.is_some_and(|state| state.trim() == "down"),
            })
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// The whole disks in `dir`: those that are not partitions.
fn disks(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| !entry.path().join("partition").exists())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// What is known of the machine's parts while its PCI functions are told.
struct Known {
    cards: Vec<Card>,
    roots: Vec<RootHub>,
    disks: Vec<String>,
    /// How many functions of the SATA and the NVMe class there are.
    ahci: usize,
    nvme: usize,
    /// Whether a USB interface is taken by a mass storage driver, which
    /// names its disk `sd` too.
    usb_disks: bool,
}

/// The device a PCI function is.
fn pci_device(f: &Function, names: &lspci::Names, known: &Known) -> Device {
    let address = f.address();
    let class = (f.base_class(), f.subclass());
    let code =
        u32::from(f.base_class()) << 16 | u32::from(f.subclass()) << 8 | u32::from(f.interface());
    let group = match class {
        USB_CONTROLLER => Group::Usb,
        (NETWORK, _) => Group::Network,
        (STORAGE, _) => Group::Storage,
        (DISPLAY, _) => Group::Display,
        _ => Group::Other,
    };
    let vendor = names.vendors.get(&f.vendor).map(String::as_str);
    let device = names.devices.get(&(f.vendor, f.device)).map(String::as_str);
    let mut found = Device {
        group,
        name: vendor_device(vendor, device, (f.vendor, f.device), Ids::Names),
        ids: format!("{:04x}:{:04x}", f.vendor, f.device),
        class: names.class(f).map(str::to_string),
        system: Vec::new(),
        driver: None,
        driver_told: false,
        state: None,
        place: format!("PCI {address}"),
        hidden: f.base_class() == BRIDGE,
    };
    let told = |found: &mut Device, driver: &str, system: Vec<String>| {
        found.driver = Some(driver.to_string());
        found.driver_told = true;
        found.state = (!system.is_empty()).then_some(State::Working);
        found.system = system;
    };
    match (group, code) {
        (Group::Network, _) => {
            if let Some(card) =
                known.cards.iter().find(|card| card.bus.as_deref() == Some(&address))
            {
                found.driver.clone_from(&card.driver);
                found.driver_told = card.driver.is_some();
                found.state = Some(if card.link_down { State::NoLink } else { State::Working });
                found.system = vec![card.name.clone()];
            }
        }
        (_, AHCI_CLASS) => {
            let mine = if known.ahci == 1 && !known.usb_disks {
                known.disks.iter().filter(|disk| disk.starts_with("sd")).cloned().collect()
            } else {
                Vec::new()
            };
            told(&mut found, "ahci", mine);
        }
        (_, NVME_CLASS) => {
            let mine = if known.nvme == 1 {
                known.disks.iter().filter(|disk| disk.starts_with("nvme")).cloned().collect()
            } else {
                Vec::new()
            };
            told(&mut found, "nvme", mine);
        }
        (Group::Usb, _) => {
            let roots: Vec<&RootHub> = known
                .roots
                .iter()
                .filter(|root| root.controller.as_deref() == Some(&address))
                .collect();
            if let Some(first) = roots.first() {
                let system = roots.iter().map(|root| format!("usb{}", root.bus)).collect();
                told(&mut found, first.driver, system);
            }
        }
        _ => {}
    }
    found
}

/// The device a USB device is: named as lsusb names it, with the drivers of
/// its interfaces.
fn usb_device(d: &UsbDevice, names: &lsusb::Names) -> Device {
    let vendor = names.vendors.get(&d.vendor).or(d.manufacturer.as_ref());
    let product = names.products.get(&(d.vendor, d.product)).or(d.name.as_ref());
    let name = match (vendor, product) {
        (Some(vendor), Some(product)) if product.starts_with(vendor.as_str()) => product.clone(),
        (Some(vendor), Some(product)) => format!("{vendor} {product}"),
        (None, Some(product)) => product.clone(),
        (Some(vendor), None) => format!("{vendor} device"),
        (None, None) => format!("USB device {:04x}:{:04x}", d.vendor, d.product),
    };
    let mut drivers: Vec<&str> = Vec::new();
    for driver in d.interfaces.iter().filter_map(|interface| interface.driver.as_deref()) {
        if !drivers.contains(&driver) {
            drivers.push(driver);
        }
    }
    let ports: Vec<String> = d.ports.iter().map(u8::to_string).collect();
    Device {
        group: Group::Usb,
        name,
        ids: format!("{:04x}:{:04x}", d.vendor, d.product),
        class: d
            .interfaces
            .first()
            .and_then(|interface| names.classes.get(&interface.class))
            .cloned(),
        system: Vec::new(),
        driver: (!drivers.is_empty()).then(|| drivers.join(", ")),
        driver_told: true,
        state: Some(if drivers.is_empty() { State::NoDriver } else { State::Working }),
        place: format!("USB {}-{}", d.bus, ports.join(".")),
        hidden: d.hub_ports > 0 || d.interfaces.iter().any(|interface| interface.class == HUB),
    }
}

/// What the machine's devices are, read from `sources`.
pub(crate) fn gather(sources: &Sources) -> Devices {
    let functions = lspci::functions(&sources.pci).unwrap_or_default();
    let (roots, usb) = lsusb::read_devices(&sources.usb).unwrap_or_default();
    let pci_names =
        lspci::Names::read(&fs::read_to_string(&sources.pci_ids).unwrap_or_default(), &functions);
    let usb_names =
        lsusb::Names::read(&fs::read_to_string(&sources.usb_ids).unwrap_or_default(), &usb);
    let count = |class: u32| {
        functions
            .iter()
            .filter(|f| {
                u32::from(f.base_class()) << 16
                    | u32::from(f.subclass()) << 8
                    | u32::from(f.interface())
                    == class
            })
            .count()
    };
    let known = Known {
        cards: cards(&sources.net),
        disks: disks(&sources.blocks),
        ahci: count(AHCI_CLASS),
        nvme: count(NVME_CLASS),
        usb_disks: usb
            .iter()
            .flat_map(|d| &d.interfaces)
            .any(|interface| matches!(interface.driver.as_deref(), Some("usb-storage" | "uas"))),
        roots,
    };
    let mut devices: Vec<Device> =
        functions.iter().map(|f| pci_device(f, &pci_names, &known)).collect();
    devices.extend(usb.iter().map(|d| usb_device(d, &usb_names)));
    Devices { format: 1, devices }
}

impl Devices {
    /// The JSON the command prints, on one line.
    ///
    /// # Errors
    ///
    /// The words of the writer's refusal, which a document of these types
    /// does not give.
    pub(crate) fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// The text a terminal shows: each group, a line for each device in
    /// it, then how many bridges and hubs are left out.
    pub(crate) fn text(&self) -> String {
        let groups = [
            (Group::Network, "Network"),
            (Group::Storage, "Storage"),
            (Group::Usb, "USB"),
            (Group::Display, "Display"),
            (Group::Other, "Other"),
        ];
        let mut text = String::new();
        for (group, title) in groups {
            let shown: Vec<&Device> =
                self.devices.iter().filter(|d| d.group == group && !d.hidden).collect();
            if shown.is_empty() {
                continue;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            let _ = writeln!(text, "{title}");
            for device in shown {
                let _ = writeln!(text, "  {}", device.line());
            }
        }
        let hidden = self.devices.iter().filter(|d| d.hidden).count();
        if hidden > 0 {
            let _ = write!(
                text,
                "\n{hidden} bridges and hubs that work and need nothing are not listed.\n"
            );
        }
        if text.is_empty() {
            return "The kernel found no device.\n".to_string();
        }
        text
    }
}

impl Device {
    /// The device as one line: its name, what it is to the system, its
    /// driver and state, and where it is.
    fn line(&self) -> String {
        let driver = match (&self.driver, self.driver_told) {
            (Some(driver), _) => format!("driver {driver}"),
            (None, true) => "no driver".to_string(),
            (None, false) => "driver not told".to_string(),
        };
        let state = match self.state {
            Some(State::Working) => ", working",
            Some(State::NoLink) => ", no link",
            Some(State::NoDriver) | None => "",
        };
        let system = if self.system.is_empty() {
            String::new()
        } else {
            format!(" as {}", self.system.join(", "))
        };
        format!("{} ({}){system}: {driver}{state}; {}", self.name, self.ids, self.place)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    /// A folder with the machine's layout, which a test fills in.
    struct Machine {
        root: PathBuf,
    }

    impl Machine {
        fn new(call: &str) -> Machine {
            let root = std::env::temp_dir()
                .join(format!("hardware-devices-{call}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Machine { root }
        }

        fn sources(&self) -> Sources {
            Sources {
                pci: self.root.join("pci"),
                usb: self.root.join("usb"),
                net: self.root.join("net"),
                blocks: self.root.join("block"),
                pci_ids: self.root.join("pci.ids"),
                usb_ids: self.root.join("usb.ids"),
            }
        }

        fn put(&self, path: &str, text: &str) {
            let file = self.root.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        }

        /// A PCI function.
        fn function(&self, address: &str, vendor: u16, device: u16, class: u32) {
            self.put(&format!("pci/{address}/vendor"), &format!("{vendor:#06x}\n"));
            self.put(&format!("pci/{address}/device"), &format!("{device:#06x}\n"));
            self.put(&format!("pci/{address}/class"), &format!("{class:#08x}\n"));
        }

        fn disk(&self, name: &str) {
            self.put(&format!("block/{name}/size"), "100\n");
        }

        /// A network card on `bus`, whose link is `state`.
        fn card(&self, name: &str, driver: &str, bus: &str, state: &str) {
            self.put(
                &format!("net/{name}/card"),
                &format!("driver {driver}\nmodel Intel I210\nbus {bus}\nreceive_slots 256\n"),
            );
            self.put(&format!("net/{name}/operstate"), &format!("{state}\n"));
        }

        fn usb_device(&self, name: &str, ids: (u16, u16), hub_ports: u8, product: &str) {
            let bus: u8 = name.split('-').next().unwrap().parse().unwrap();
            self.put(&format!("usb/{name}/idVendor"), &format!("{:04x}\n", ids.0));
            self.put(&format!("usb/{name}/idProduct"), &format!("{:04x}\n", ids.1));
            self.put(&format!("usb/{name}/devnum"), &format!("{}\n", 2 + hub_ports));
            self.put(&format!("usb/{name}/busnum"), &format!("{bus}\n"));
            self.put(&format!("usb/{name}/maxchild"), &format!("{hub_ports}\n"));
            self.put(&format!("usb/{name}/speed"), "480\n");
            self.put(&format!("usb/{name}/product"), &format!("{product}\n"));
        }

        /// An interface of a USB device, taken by `driver` or by none.
        fn interface(&self, device: &str, class: u8, driver: Option<&str>) {
            let name = format!("{device}:1.0");
            self.put(&format!("usb/{name}/bInterfaceNumber"), "00\n");
            self.put(&format!("usb/{name}/bInterfaceClass"), &format!("{class:02x}\n"));
            if let Some(driver) = driver {
                self.put(&format!("usbdrivers/{driver}/.keep"), "");
                std::os::unix::fs::symlink(
                    self.root.join("usbdrivers").join(driver),
                    self.root.join("usb").join(&name).join("driver"),
                )
                .unwrap();
            }
        }

        fn root_hub(&self, bus: u8, product: &str, controller: &str) {
            let name = format!("usb{bus}");
            self.put(&format!("usb/{name}/maxchild"), "4\n");
            self.put(&format!("usb/{name}/speed"), "5000\n");
            self.put(&format!("usb/{name}/product"), &format!("{product}\n"));
            self.put(&format!("usb/{name}/serial"), &format!("{controller}\n"));
        }
    }

    fn found(machine: &Machine) -> Value {
        serde_json::from_str(&gather(&machine.sources()).json().unwrap()).unwrap()
    }

    fn by_place<'a>(value: &'a Value, place: &str) -> &'a Value {
        value["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|device| device["where"] == json!(place))
            .unwrap_or_else(|| panic!("no device at {place}: {value}"))
    }

    #[test]
    fn a_machine_with_nothing_has_no_devices() {
        let machine = Machine::new("nothing");
        let devices = gather(&machine.sources());
        assert_eq!(
            serde_json::from_str::<Value>(&devices.json().unwrap()).unwrap(),
            json!({"format": 1, "devices": []})
        );
        assert_eq!(devices.text(), "The kernel found no device.\n");
    }

    #[test]
    fn a_network_card_has_its_driver_its_name_and_its_link() {
        let machine = Machine::new("network");
        machine.function("0000:03:00.0", 0x8086, 0x1533, 0x020000);
        machine.function("0000:04:00.0", 0x8086, 0x1533, 0x020000);
        machine.function("0000:05:00.0", 0x10ec, 0x8168, 0x020000);
        machine.card("net1", "radix-intel-net", "0000:03:00.0", "up");
        machine.card("net2", "radix-intel-net", "0000:04:00.0", "down");
        machine.put("pci.ids", "8086  Intel Corporation\n\t1533  I210 Gigabit Network Connection\nC 02  Network controller\n\t00  Ethernet controller\n");
        let value = found(&machine);
        assert_eq!(
            by_place(&value, "PCI 0000:03:00.0"),
            &json!({
                "group": "network", "name": "Intel Corporation I210 Gigabit Network Connection",
                "ids": "8086:1533", "class": "Ethernet controller", "system": ["net1"],
                "driver": "radix-intel-net", "driver_told": true, "state": "working",
                "where": "PCI 0000:03:00.0", "hidden": false,
            })
        );
        assert_eq!(by_place(&value, "PCI 0000:04:00.0")["state"], json!("no-link"));
        // A card no driver of the kernel's lists: not told, never "none".
        let unknown = by_place(&value, "PCI 0000:05:00.0");
        assert_eq!(unknown["driver"], Value::Null);
        assert_eq!(unknown["driver_told"], json!(false));
        assert_eq!(unknown["state"], Value::Null);
        assert_eq!(unknown["name"], json!("Device 10ec:8168"));
        assert_eq!(unknown["system"], json!([]));
    }

    #[test]
    fn the_controllers_of_the_classes_the_kernel_binds_whole_are_told() {
        let machine = Machine::new("storage");
        machine.function("0000:00:17.0", 0x8086, 0xa102, 0x010601);
        machine.function("0000:01:00.0", 0x144d, 0xa808, 0x010802);
        machine.function("0000:02:00.0", 0x1000, 0x0072, 0x010700);
        for disk in ["sda", "sdb", "nvme0n1"] {
            machine.disk(disk);
        }
        machine.put("block/sda1/partition", "1\n");
        let value = found(&machine);
        let sata = by_place(&value, "PCI 0000:00:17.0");
        assert_eq!(sata["driver"], json!("ahci"));
        assert_eq!(sata["system"], json!(["sda", "sdb"]));
        assert_eq!(
            (sata["state"].clone(), sata["group"].clone()),
            (json!("working"), json!("storage"))
        );
        let nvme = by_place(&value, "PCI 0000:01:00.0");
        assert_eq!(
            (nvme["driver"].clone(), nvme["system"].clone()),
            (json!("nvme"), json!(["nvme0n1"]))
        );
        // The SAS class is not one the kernel binds whole: not told.
        let other = by_place(&value, "PCI 0000:02:00.0");
        assert_eq!(
            (other["driver"].clone(), other["driver_told"].clone()),
            (Value::Null, json!(false))
        );
    }

    #[test]
    fn disks_are_named_on_a_controller_only_where_the_names_leave_no_doubt() {
        // Two SATA controllers: which has sda is not told.
        let machine = Machine::new("two");
        machine.function("0000:00:17.0", 0x8086, 0xa102, 0x010601);
        machine.function("0000:05:00.0", 0x1b21, 0x0612, 0x010601);
        machine.disk("sda");
        let value = found(&machine);
        for place in ["PCI 0000:00:17.0", "PCI 0000:05:00.0"] {
            let controller = by_place(&value, place);
            assert_eq!(controller["driver"], json!("ahci"));
            assert_eq!(
                (controller["system"].clone(), controller["state"].clone()),
                (json!([]), Value::Null)
            );
        }
        // One, but a USB stick is named `sd` too.
        let machine = Machine::new("stick");
        machine.function("0000:00:17.0", 0x8086, 0xa102, 0x010601);
        machine.disk("sda");
        machine.disk("sdb");
        machine.usb_device("1-2", (0x0781, 0x5567), 0, "Cruzer Blade");
        machine.interface("1-2", 8, Some("usb-storage"));
        assert_eq!(by_place(&found(&machine), "PCI 0000:00:17.0")["system"], json!([]));
    }

    #[test]
    fn a_usb_controller_is_told_by_its_root_hubs() {
        let machine = Machine::new("controller");
        machine.function("0000:00:14.0", 0x8086, 0xa12f, 0x0c0330);
        machine.function("0000:00:1d.0", 0x8086, 0xa110, 0x060400);
        machine.root_hub(1, "xHCI Host Controller", "0000:00:14.0");
        machine.root_hub(2, "xHCI Host Controller", "0000:00:14.0");
        let value = found(&machine);
        let controller = by_place(&value, "PCI 0000:00:14.0");
        assert_eq!(controller["group"], json!("usb"));
        assert_eq!(controller["driver"], json!("xhci_hcd"));
        assert_eq!(controller["system"], json!(["usb1", "usb2"]));
        assert_eq!(controller["state"], json!("working"));
        // A bridge is hidden, with no driver told.
        let bridge = by_place(&value, "PCI 0000:00:1d.0");
        assert_eq!(
            (bridge["hidden"].clone(), bridge["driver_told"].clone()),
            (json!(true), json!(false))
        );
    }

    #[test]
    fn usb_devices_say_their_interfaces_drivers_and_none_where_none_took_them() {
        let machine = Machine::new("usb");
        machine.root_hub(1, "xHCI Host Controller", "0000:00:14.0");
        machine.usb_device("1-1", (0x046d, 0xc52b), 0, "USB Receiver");
        machine.interface("1-1", 3, Some("usbhid"));
        machine.usb_device("1-2", (0x1234, 0x0001), 0, "Strange Gadget");
        machine.interface("1-2", 0xff, None);
        machine.usb_device("1-3", (0x05e3, 0x0608), 4, "USB2.0 Hub");
        machine.interface("1-3", 9, Some("hub"));
        machine.put("usb.ids", "046d  Logitech, Inc.\n\tc52b  Unifying Receiver\nC 03  Human Interface Device\nC 09  Hub\n");
        let value = found(&machine);
        assert_eq!(
            by_place(&value, "USB 1-1"),
            &json!({
                "group": "usb", "name": "Logitech, Inc. Unifying Receiver", "ids": "046d:c52b",
                "class": "Human Interface Device", "system": [], "driver": "usbhid",
                "driver_told": true, "state": "working", "where": "USB 1-1", "hidden": false,
            })
        );
        // No driver link: the system knows none took it.
        let gadget = by_place(&value, "USB 1-2");
        assert_eq!(
            (gadget["driver"].clone(), gadget["driver_told"].clone()),
            (Value::Null, json!(true))
        );
        assert_eq!(gadget["state"], json!("no-driver"));
        assert_eq!(gadget["name"], json!("Strange Gadget"));
        // A hub is hidden; the root hub is not a row of its own.
        assert_eq!(by_place(&value, "USB 1-3")["hidden"], json!(true));
        assert_eq!(value["devices"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn the_text_groups_the_devices_and_counts_what_it_folds() {
        let machine = Machine::new("text");
        machine.function("0000:03:00.0", 0x8086, 0x1533, 0x020000);
        machine.function("0000:00:1d.0", 0x8086, 0xa110, 0x060400);
        machine.function("0000:00:02.0", 0x8086, 0x1912, 0x030000);
        machine.card("net1", "radix-intel-net", "0000:03:00.0", "down");
        let devices = gather(&machine.sources());
        let text = devices.text();
        assert!(text.starts_with("Network\n  Device 8086:1533 (8086:1533) as net1: driver radix-intel-net, no link; PCI 0000:03:00.0\n"), "{text}");
        assert!(
            text.contains(
                "\nDisplay\n  Device 8086:1912 (8086:1912): driver not told; PCI 0000:00:02.0\n"
            ),
            "{text}"
        );
        assert!(
            text.ends_with("\n1 bridges and hubs that work and need nothing are not listed.\n"),
            "{text}"
        );
        assert_eq!(devices.devices.len(), 3);
    }
}
