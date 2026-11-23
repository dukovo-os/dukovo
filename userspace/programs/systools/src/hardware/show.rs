// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What this machine is, which `hardware show` says: its maker, model and
//! chassis, its processor, its memory and its firmware.
//!
//! Everyone may read most of it: the firmware's strings in
//! `/kernel/class/dmi/id`, the processor's own CPUID, and what the kernel
//! counts of CPUs and memory. What the memory is made of, its modules and
//! whether it corrects errors, is in SMBIOS's tables, which a program
//! reads whole only with the platform capability that admin.hardware gives
//! (hwinfo(8)). Without them those members are `null`, which says they were
//! not read; with them `modules` lists the modules that are there.
//!
//! No serial number or UUID is read, so that the answer can be shown and
//! put in a report as it is.

use std::fmt::Write as _;

use radix_smbios::memory::{self, MemoryArray, MemoryDevice};
use radix_smbios::processor::Processor as SmbiosProcessor;
use radix_smbios::{Entry, Table, chassis_name, kind};
use serde::Serialize;

use crate::admin_about::{Sources, dmi};
use crate::hwinfo::{self, Tables};
use crate::ipmi;
use crate::lscpu::Summary;
use crate::{dukofetch, proc};

/// The kernel's count of the CPUs it runs.
const CPUS: &str = "/kernel/cpus";

/// What `hardware show --json` says, in this order. A member the machine
/// does not say is `null`.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Show {
    /// The version of this document: 1.
    format: u32,
    /// The firmware's name for the maker of the machine.
    maker: Option<String>,
    /// Its name for the model.
    model: Option<String>,
    /// The kind of case, as SMBIOS names it: `Mini Tower`, `Notebook`.
    chassis: Option<String>,
    processor: Processor,
    memory: Memory,
    firmware: Firmware,
    /// The baseboard management controller; left out of the document on a
    /// machine with none that the caller may ask.
    #[serde(skip_serializing_if = "Option::is_none")]
    bmc: Option<Bmc>,
}

/// The machine's baseboard management controller, as it says itself.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Bmc {
    /// Its maker: `Dell`, or `enterprise 12345` for a number the program
    /// has no name for.
    maker: String,
    firmware: String,
    /// `2.0`.
    ipmi_version: String,
    /// The address of its network port; null when it has no port or no
    /// address.
    lan_address: Option<String>,
}

/// The processor, as its own CPUID and the kernel say it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Processor {
    /// Its name without its marks, as `Intel Core i5-6500`.
    model: Option<String>,
    /// The sockets with a processor in them, as the firmware's tables say;
    /// null when they are not read or list none.
    sockets: Option<u32>,
    /// The cores, all the machine's processors together.
    cores: Option<u32>,
    /// The CPUs the kernel runs, which a core may run two or more of.
    threads: Option<u32>,
    /// The clock it is rated at, in MHz.
    base_mhz: Option<u32>,
    /// The highest clock it runs at, in MHz, when it says.
    max_mhz: Option<u32>,
}

/// The memory: what the kernel counts, and what the tables say of it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Memory {
    /// What the kernel counts, in bytes.
    bytes: Option<u64>,
    /// Whether the memory corrects errors; null when the tables are not
    /// read or do not say.
    ecc: Option<bool>,
    /// How many slots the board has, empty ones too; null when the tables
    /// are not read.
    slots: Option<usize>,
    /// The modules that are there; null when the tables are not read.
    modules: Option<Vec<Module>>,
}

/// A memory module in a slot.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Module {
    /// The slot, as the board names it: `DIMM A1`.
    slot: String,
    /// Its size in bytes, when the firmware says.
    bytes: Option<u64>,
    /// The kind of memory: `DDR4`.
    kind: String,
    /// The speed it runs at, in MT/s: what is set, else what it is
    /// rated for.
    speed: Option<u32>,
    /// Who made it.
    maker: Option<String>,
    /// Its part number.
    part: Option<String>,
}

/// The firmware.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Firmware {
    vendor: Option<String>,
    version: Option<String>,
    /// As the firmware writes it, `07/12/2023`.
    date: Option<String>,
}

/// The sockets the tables list with a processor in them, if they list
/// any that has one.
fn sockets(tables: &Tables) -> Option<u32> {
    let entry = Entry::parse(&tables.entry)?;
    let table = Table::new(&tables.structures, entry.count);
    let populated = table
        .of(kind::PROCESSOR)
        .filter_map(|s| SmbiosProcessor::parse(&s))
        .filter(|processor| processor.populated)
        .count();
    u32::try_from(populated).ok().filter(|&n| n > 0)
}

/// What the tables say of the memory.
struct Installed {
    ecc: Option<bool>,
    slots: usize,
    modules: Vec<Module>,
}

/// What this machine says of itself: `sources` for the firmware's strings
/// and the kernel's counts, `cpu` for the processor, and `tables` when
/// they were read.
pub(crate) fn gather(sources: &Sources, cpu: &Summary, tables: Option<&Tables>) -> Show {
    let threads =
        sources.read(CPUS).and_then(|text| text.trim().parse::<u32>().ok()).filter(|&n| n > 0);
    let bytes = sources
        .read(format!("{}/meminfo", proc::PROC))
        .map(|text| proc::parse_memory(text.as_bytes()).total * 1024)
        .filter(|&bytes| bytes > 0);
    let installed = tables.and_then(installed);
    let (ecc, slots, modules) = match installed {
        Some(found) => (found.ecc, Some(found.slots), Some(found.modules)),
        None => (None, None, None),
    };
    Show {
        format: 1,
        maker: dmi(sources, "sys_vendor"),
        model: dmi(sources, "product_name"),
        chassis: dmi(sources, "chassis_type")
            .and_then(|kind| kind.parse().ok())
            .and_then(chassis_name)
            .map(String::from),
        processor: Processor {
            model: cpu.name.as_deref().map(dukofetch::cpu_name).filter(|name| !name.is_empty()),
            sockets: tables.and_then(sockets),
            cores: threads.map(|threads| threads.div_ceil(cpu.threads_per_core.max(1))),
            threads,
            base_mhz: cpu.base_mhz,
            max_mhz: cpu.max_mhz,
        },
        memory: Memory { bytes, ecc, slots, modules },
        firmware: Firmware {
            vendor: dmi(sources, "bios_vendor"),
            version: dmi(sources, "bios_version"),
            date: dmi(sources, "bios_date"),
        },
        bmc: None,
    }
}

/// The BMC that `link` talks to.
///
/// # Errors
///
/// If the BMC does not answer.
pub(crate) fn bmc_of(link: &mut dyn ipmi::Transport) -> Result<Bmc, ipmi::Error> {
    let id = ipmi::device_id(link)?;
    let lan = match ipmi::find_lan_channel(link)? {
        Some(channel) => Some(ipmi::read_lan(link, channel)?),
        None => None,
    };
    Ok(Bmc {
        maker: id.manufacturer_name(),
        firmware: id.firmware,
        ipmi_version: id.ipmi_version,
        lan_address: lan.and_then(|lan| lan.address).map(|address| address.to_string()),
    })
}

/// The BMC as this machine has it for the caller: none when there is none
/// or the caller may not open its interface, and none, with the reason
/// said, when it does not answer.
pub(crate) fn bmc_of_machine() -> Option<Bmc> {
    let mut device = match ipmi::Device::open_if_allowed() {
        Ok(device) => device?,
        Err(error) => {
            eprintln!("hardware: cannot open the BMC: {}", crate::util::strip_os_error(&error));
            return None;
        }
    };
    match bmc_of(&mut device) {
        Ok(bmc) => Some(bmc),
        Err(error) => {
            eprintln!("hardware: the BMC is not shown: {error}");
            None
        }
    }
}

/// The text of a string of the tables, if it says anything.
fn said(text: Option<radix_smbios::Str<'_>>) -> Option<String> {
    let text = text?.to_string();
    Some(text.trim().to_string()).filter(|text| !text.is_empty())
}

/// What the tables say of the memory, if the entry point reads. The
/// first memory array tells whether errors are corrected, as hwinfo says
/// it; each memory device is a slot, and a module is a slot that is not
/// empty.
fn installed(tables: &Tables) -> Option<Installed> {
    let entry = Entry::parse(&tables.entry)?;
    let table = Table::new(&tables.structures, entry.count);
    let ecc = table.of(kind::MEMORY_ARRAY).find_map(|s| MemoryArray::parse(&s)).and_then(|array| {
        match array.ecc {
            // Single-bit and multi-bit ECC correct; none and parity do not.
            0x05 | 0x06 => Some(true),
            0x03 | 0x04 => Some(false),
            _ => None,
        }
    });
    let devices: Vec<MemoryDevice<'_>> =
        table.of(kind::MEMORY_DEVICE).filter_map(|s| MemoryDevice::parse(&s)).collect();
    let modules = devices
        .iter()
        .enumerate()
        .filter(|(_, device)| device.is_present())
        .map(|(at, device)| Module {
            slot: said(device.locator).unwrap_or_else(|| format!("Slot {}", at + 1)),
            bytes: device.size,
            kind: memory::kind_name(device.kind).to_string(),
            speed: device.configured_speed.or(device.speed),
            maker: said(device.manufacturer),
            part: said(device.part_number),
        })
        .collect();
    Some(Installed { ecc, slots: devices.len(), modules })
}

/// `bytes` as a person reads the memory they have: `15.6 GiB`.
fn amount(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MiB", bytes >> 20)
    }
}

/// `mhz` as a clock in GHz, with the digits that matter: `3.2 GHz`.
fn clock(mhz: u32) -> String {
    let ghz = format!("{:.2}", f64::from(mhz) / 1000.0);
    format!("{} GHz", ghz.trim_end_matches('0').trim_end_matches('.'))
}

impl Show {
    /// What the machine says, with its BMC when it has one.
    #[must_use]
    pub(crate) fn with_bmc(mut self, bmc: Option<Bmc>) -> Show {
        self.bmc = bmc;
        self
    }

    /// The JSON the command prints.
    ///
    /// # Errors
    ///
    /// The words of the writer's refusal, which a document of these types
    /// does not give.
    pub(crate) fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// The text a terminal shows: a line for each thing the machine said,
    /// as `Label  value`, with a line under Memory for each module.
    pub(crate) fn text(&self) -> String {
        let mut rows: Vec<(&str, String)> = Vec::new();
        let machine: Vec<&str> =
            [&self.maker, &self.model].into_iter().flatten().map(String::as_str).collect();
        if !machine.is_empty() {
            let case = self.chassis.as_ref().map_or(String::new(), |case| format!(" ({case})"));
            rows.push(("Machine", format!("{}{case}", machine.join(" "))));
        }
        let processor = &self.processor;
        // A machine of two or more sockets: `2 × Intel Xeon Gold 6152`.
        let model = processor.model.as_ref().map(|model| match processor.sockets {
            Some(sockets) if sockets > 1 => format!("{sockets} × {model}"),
            _ => model.clone(),
        });
        let mut parts: Vec<String> = model.into_iter().collect();
        if let Some(cores) = processor.cores {
            parts.push(format!("{cores} core{}", if cores == 1 { "" } else { "s" }));
        }
        if let Some(threads) = processor.threads.filter(|threads| Some(*threads) != processor.cores)
        {
            parts.push(format!("{threads} threads"));
        }
        match (processor.base_mhz, processor.max_mhz) {
            (Some(base), Some(max)) if max > base => {
                parts.push(format!("{}, up to {}", clock(base), clock(max)));
            }
            (Some(mhz), _) | (None, Some(mhz)) => parts.push(clock(mhz)),
            (None, None) => {}
        }
        if !parts.is_empty() {
            rows.push(("Processor", parts.join(", ")));
        }
        let memory = &self.memory;
        let mut parts: Vec<String> = memory.bytes.map(amount).into_iter().collect();
        match memory.ecc {
            Some(true) => parts.push("corrects errors".into()),
            Some(false) => parts.push("no error correction".into()),
            None => {}
        }
        if let (Some(slots), Some(modules)) = (memory.slots, &memory.modules) {
            parts.push(format!("{} of {slots} slots used", modules.len()));
        }
        if !parts.is_empty() {
            rows.push(("Memory", parts.join(", ")));
        }
        for module in memory.modules.iter().flatten() {
            let mut parts: Vec<String> = Vec::new();
            let what = match module.bytes {
                Some(bytes) => format!("{} {}", hwinfo::size(bytes), module.kind),
                None => module.kind.clone(),
            };
            parts.push(what);
            parts.extend(module.speed.map(|speed| format!("{speed} MT/s")));
            let made: Vec<&str> =
                [&module.maker, &module.part].into_iter().flatten().map(String::as_str).collect();
            if !made.is_empty() {
                parts.push(made.join(" "));
            }
            rows.push(("Module", format!("{}: {}", module.slot, parts.join(", "))));
        }
        let firmware = &self.firmware;
        let parts: Vec<&str> = [&firmware.vendor, &firmware.version, &firmware.date]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        if !parts.is_empty() {
            rows.push(("Firmware", parts.join(", ")));
        }
        if let Some(bmc) = &self.bmc {
            rows.push((
                "BMC",
                format!("{}, firmware {}, IPMI {}", bmc.maker, bmc.firmware, bmc.ipmi_version),
            ));
            if let Some(address) = &bmc.lan_address {
                rows.push(("BMC LAN", address.clone()));
            }
        }
        let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
        rows.iter().fold(String::new(), |mut text, (label, value)| {
            let _ = writeln!(text, "{label:<width$}  {value}");
            text
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use serde_json::{Value, json};

    use super::*;

    /// A folder with the machine's layout, which a test fills in.
    struct Machine {
        root: PathBuf,
    }

    impl Machine {
        fn new(call: &str) -> Machine {
            let root =
                std::env::temp_dir().join(format!("hardware-show-{call}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Machine { root }
        }

        fn sources(&self) -> Sources {
            Sources::under(&self.root)
        }

        fn put(&self, path: &str, text: &str) {
            let file = self.root.join(path.trim_start_matches('/'));
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        }

        /// What anyone may read of an OptiPlex, and the serial numbers
        /// that only root may, which nothing here may print.
        fn optiplex(&self) {
            for (name, value) in [
                ("sys_vendor", "Dell Inc."),
                ("product_name", "OptiPlex 7040"),
                ("chassis_type", "6"),
                ("bios_vendor", "Dell Inc."),
                ("bios_version", "1.24.0"),
                ("bios_date", "07/12/2023"),
                ("product_serial", "SERIAL-SYSTEM"),
                ("board_serial", "SERIAL-BOARD"),
                ("chassis_serial", "SERIAL-CHASSIS"),
                ("product_uuid", "4c4c4544-0042-3910-8051-b7c04f4e3332"),
            ] {
                self.put(&format!("kernel/class/dmi/id/{name}"), &format!("{value}\n"));
            }
            self.put("kernel/cpus", "4\n");
            self.put("processes/meminfo", "total: 16318464 kB\nfree: 100 kB\n");
        }
    }

    /// An i5-6500, which a core runs one CPU of.
    fn i5() -> Summary {
        Summary {
            name: Some("Intel(R) Core(TM) i5-6500 CPU @ 3.20GHz".into()),
            threads_per_core: 1,
            base_mhz: Some(3200),
            max_mhz: Some(3600),
        }
    }

    /// A structure of type `kind`, as the firmware lays it out.
    fn structure(kind: u8, fields: &[u8], strings: &[&str]) -> Vec<u8> {
        let mut bytes = vec![kind, (4 + fields.len()) as u8, 0, 0];
        bytes.extend_from_slice(fields);
        for string in strings {
            bytes.extend_from_slice(string.as_bytes());
            bytes.push(0);
        }
        if strings.is_empty() {
            bytes.push(0);
        }
        bytes.push(0);
        bytes
    }

    /// A 64-bit entry point of SMBIOS 3.0.0.
    fn entry() -> Vec<u8> {
        let mut e = b"_SM3_".to_vec();
        e.extend_from_slice(&[0, 0x18, 3, 0, 0, 1, 0]);
        e.extend_from_slice(&0x1000u32.to_le_bytes());
        e.extend_from_slice(&0x7f00_0000u64.to_le_bytes());
        let sum = e.iter().fold(0u8, |s, b| s.wrapping_add(*b));
        e[5] = 0u8.wrapping_sub(sum);
        e
    }

    /// A memory device of `size` MiB; with `strings` it has its slot, its
    /// maker, its serial number and its part, else its slot alone.
    fn device(size: u16, strings: bool, speed: u16, configured: u16) -> Vec<u8> {
        let mut d = vec![0x00, 0x10, 0xfe, 0xff, 64, 0, 64, 0];
        d.extend_from_slice(&size.to_le_bytes());
        d.extend_from_slice(&[0x09, 0, 1, 2, 0x1a, 0x80, 0]);
        d.extend_from_slice(&speed.to_le_bytes());
        d.extend_from_slice(if strings { &[3, 4, 0, 5, 1] } else { &[0, 0, 0, 0, 0] });
        // The rest of the formatted area up to the configured speed.
        d.extend_from_slice(&[0; 0x20 - 0x1c]);
        d.extend_from_slice(&configured.to_le_bytes());
        d
    }

    /// The tables of a board with four slots, two of them filled, and
    /// memory that corrects `ecc` errors in the way of the byte.
    fn tables(ecc: u8) -> Tables {
        let mut t = Vec::new();
        let mut array = vec![3, 3, ecc];
        array.extend_from_slice(&(64u32 << 20).to_le_bytes());
        array.extend_from_slice(&[0xfe, 0xff, 4, 0]);
        t.extend(structure(16, &array, &[]));
        t.extend(structure(4, &processor(true), &["CPU 1"]));
        let strings = |slot, bank| [slot, bank, "Samsung", "SERIAL-DIMM", "M378A1K43BB2-CRC"];
        t.extend(structure(17, &device(8192, true, 2133, 2133), &strings("DIMM A1", "BANK 0")));
        t.extend(structure(17, &device(0, false, 0, 0), &["DIMM A2", "BANK 1"]));
        t.extend(structure(17, &device(8192, true, 2400, 0), &strings("DIMM B1", "BANK 2")));
        t.extend(structure(17, &device(0, false, 0, 0), &["DIMM B2", "BANK 3"]));
        t.extend(structure(127, &[], &[]));
        Tables { entry: entry(), structures: t }
    }

    /// A processor's structure (type 4): its socket's name is string 1,
    /// and the status at 0x18 has bit 6 set when a processor is in it.
    /// `fields` starts after the 4-byte header, so offset `n` is `n - 4`.
    fn processor(populated: bool) -> Vec<u8> {
        // The socket designation (0x04) and the status (0x18).
        const SOCKET: usize = 0;
        const STATUS: usize = 0x14;
        let mut fields = vec![0u8; 0x26];
        fields[SOCKET] = 1;
        fields[STATUS] = if populated { 0x41 } else { 0 };
        fields
    }

    fn json_of(show: &Show) -> Value {
        serde_json::from_str(&show.json().unwrap()).unwrap()
    }

    #[test]
    fn a_reader_gets_what_anyone_may_read_and_null_for_what_the_tables_say() {
        let machine = Machine::new("reader");
        machine.optiplex();
        let show = gather(&machine.sources(), &i5(), None);
        assert_eq!(
            json_of(&show),
            json!({
                "format": 1,
                "maker": "Dell Inc.",
                "model": "OptiPlex 7040",
                "chassis": "Mini Tower",
                "processor": {
                    "model": "Intel Core i5-6500", "sockets": null, "cores": 4, "threads": 4,
                    "base_mhz": 3200, "max_mhz": 3600
                },
                "memory": {
                    "bytes": 16_318_464u64 * 1024, "ecc": null, "slots": null, "modules": null
                },
                "firmware": { "vendor": "Dell Inc.", "version": "1.24.0", "date": "07/12/2023" },
            })
        );
        // The document starts with its format, and the members come in the
        // order the design gives them.
        let text = show.json().unwrap();
        assert!(text.starts_with("{\n  \"format\": 1,\n  \"maker\""), "{text}");
        let order = ["maker", "model", "chassis", "processor", "memory", "firmware"];
        let at: Vec<usize> =
            order.iter().map(|key| text.find(&format!("\"{key}\"")).unwrap()).collect();
        assert!(at.windows(2).all(|pair| pair[0] < pair[1]), "{at:?}");
        let _ = fs::remove_dir_all(&machine.root);
    }

    #[test]
    fn no_serial_number_or_uuid_is_in_either_answer() {
        let machine = Machine::new("serial");
        machine.optiplex();
        for tables in [None, Some(tables(0x03))] {
            let show = gather(&machine.sources(), &i5(), tables.as_ref());
            for text in [show.json().unwrap(), show.text()] {
                assert!(!text.contains("SERIAL"), "{text}");
                assert!(!text.contains("4c4c4544"), "{text}");
            }
        }
        let _ = fs::remove_dir_all(&machine.root);
    }

    #[test]
    fn with_the_tables_the_modules_the_slots_and_the_error_correction_are_filled() {
        let machine = Machine::new("root");
        machine.optiplex();
        let show = gather(&machine.sources(), &i5(), Some(&tables(0x03)));
        let memory = json_of(&show)["memory"].clone();
        assert_eq!(memory["ecc"], json!(false));
        assert_eq!(memory["slots"], json!(4));
        // The empty slots are no modules. A module runs at what is set,
        // else at what it is rated for, and has no serial number.
        assert_eq!(
            memory["modules"],
            json!([
                {
                    "slot": "DIMM A1", "bytes": 8u64 << 30, "kind": "DDR4", "speed": 2133,
                    "maker": "Samsung", "part": "M378A1K43BB2-CRC"
                },
                {
                    "slot": "DIMM B1", "bytes": 8u64 << 30, "kind": "DDR4", "speed": 2400,
                    "maker": "Samsung", "part": "M378A1K43BB2-CRC"
                },
            ])
        );
        let _ = fs::remove_dir_all(&machine.root);
    }

    #[test]
    fn error_correction_is_told_for_the_kinds_that_correct_and_not_for_those_that_do_not_say() {
        for (byte, ecc) in [
            (0x01, None),
            (0x02, None),
            (0x03, Some(false)),
            (0x04, Some(false)),
            (0x05, Some(true)),
            (0x06, Some(true)),
            (0x07, None),
            (0x00, None),
        ] {
            assert_eq!(installed(&tables(byte)).unwrap().ecc, ecc, "{byte:#x}");
        }
        // No array says nothing, and tables whose entry point does not read
        // are not read.
        let mut none = tables(0x06);
        none.structures = structure(127, &[], &[]);
        let found = installed(&none).unwrap();
        assert_eq!((found.ecc, found.slots, found.modules.len()), (None, 0, 0));
        let broken = Tables { entry: b"_SM3_".to_vec(), structures: Vec::new() };
        assert!(installed(&broken).is_none());
    }

    #[test]
    fn a_machine_that_says_nothing_gives_nulls_and_never_fails() {
        let machine = Machine::new("empty");
        let none = Summary { name: None, threads_per_core: 1, base_mhz: None, max_mhz: None };
        let show = gather(&machine.sources(), &none, None);
        let value = json_of(&show);
        assert_eq!(value["format"], 1);
        for key in ["maker", "model", "chassis"] {
            assert_eq!(value[key], Value::Null, "{key}");
        }
        assert_eq!(value["firmware"], json!({ "vendor": null, "version": null, "date": null }));
        assert_eq!(
            value["processor"],
            json!({
                "model": null, "sockets": null, "cores": null, "threads": null,
                "base_mhz": null, "max_mhz": null
            })
        );
        assert_eq!(value["memory"]["bytes"], Value::Null);
        assert_eq!(show.text(), "");
        // Counts that are not numbers, or are none, are not a count.
        machine.put("kernel/cpus", "many\n");
        machine.put("processes/meminfo", "total: 0\n");
        let show = gather(&machine.sources(), &none, None);
        assert_eq!((show.processor.threads, show.memory.bytes), (None, None));
        machine.put("kernel/cpus", "0\n");
        assert_eq!(gather(&machine.sources(), &none, None).processor.threads, None);
        let _ = fs::remove_dir_all(&machine.root);
    }

    #[test]
    fn the_cores_are_the_cpus_over_what_a_core_runs() {
        let machine = Machine::new("cores");
        machine.optiplex();
        machine.put("kernel/cpus", "8\n");
        let laptop = Summary { threads_per_core: 2, ..i5() };
        let show = gather(&machine.sources(), &laptop, None);
        assert_eq!((show.processor.cores, show.processor.threads), (Some(4), Some(8)));
        // A CPU left over is a core, and a count of zero threads a core runs
        // is not divided by.
        machine.put("kernel/cpus", "3\n");
        let show = gather(&machine.sources(), &laptop, None);
        assert_eq!(show.processor.cores, Some(2));
        let zero = Summary { threads_per_core: 0, ..i5() };
        assert_eq!(gather(&machine.sources(), &zero, None).processor.cores, Some(3));
        let _ = fs::remove_dir_all(&machine.root);
    }

    #[test]
    fn a_chassis_is_named_when_the_firmware_gives_a_kind_of_case_it_knows() {
        let machine = Machine::new("chassis");
        for (kind, name) in [("3", Some("Desktop")), ("10", Some("Notebook")), ("999", None)] {
            machine.put("kernel/class/dmi/id/chassis_type", kind);
            let show = gather(&machine.sources(), &i5(), None);
            assert_eq!(show.chassis.as_deref(), name, "{kind}");
        }
        machine.put("kernel/class/dmi/id/chassis_type", "laptop");
        assert_eq!(gather(&machine.sources(), &i5(), None).chassis, None);
        let _ = fs::remove_dir_all(&machine.root);
    }

    #[test]
    fn the_text_says_the_machine_the_processor_the_memory_and_the_firmware() {
        let machine = Machine::new("text");
        machine.optiplex();
        let reader = gather(&machine.sources(), &i5(), None).text();
        assert_eq!(
            reader,
            "Machine    Dell Inc. OptiPlex 7040 (Mini Tower)\n\
             Processor  Intel Core i5-6500, 4 cores, 3.2 GHz, up to 3.6 GHz\n\
             Memory     15.6 GiB\n\
             Firmware   Dell Inc., 1.24.0, 07/12/2023\n"
        );
        let root = gather(&machine.sources(), &i5(), Some(&tables(0x03))).text();
        assert_eq!(
            root,
            "Machine    Dell Inc. OptiPlex 7040 (Mini Tower)\n\
             Processor  Intel Core i5-6500, 4 cores, 3.2 GHz, up to 3.6 GHz\n\
             Memory     15.6 GiB, no error correction, 2 of 4 slots used\n\
             Module     DIMM A1: 8 GiB DDR4, 2133 MT/s, Samsung M378A1K43BB2-CRC\n\
             Module     DIMM B1: 8 GiB DDR4, 2400 MT/s, Samsung M378A1K43BB2-CRC\n\
             Firmware   Dell Inc., 1.24.0, 07/12/2023\n"
        );
        let _ = fs::remove_dir_all(&machine.root);
    }

    #[test]
    fn a_machine_of_two_sockets_says_so_before_the_model() {
        let machine = Machine::new("sockets");
        machine.optiplex();
        let of = |populated: &[bool]| {
            let mut t = Vec::new();
            for (at, &full) in populated.iter().enumerate() {
                t.extend(structure(4, &processor(full), &[&format!("CPU {}", at + 1)]));
            }
            t.extend(structure(127, &[], &[]));
            gather(&machine.sources(), &i5(), Some(&Tables { entry: entry(), structures: t }))
        };
        let two = of(&[true, true]);
        assert_eq!(json_of(&two)["processor"]["sockets"], 2);
        assert!(
            two.text().contains("Processor  2 × Intel Core i5-6500, 4 cores,"),
            "{}",
            two.text()
        );
        // An empty socket is not counted, and one processor is not said.
        let one = of(&[true, false]);
        assert_eq!(json_of(&one)["processor"]["sockets"], 1);
        assert!(one.text().contains("Processor  Intel Core i5-6500, 4 cores,"), "{}", one.text());
        // Tables that list no processor in a socket say nothing of them.
        assert_eq!(json_of(&of(&[false]))["processor"]["sockets"], Value::Null);
        assert_eq!(json_of(&of(&[]))["processor"]["sockets"], Value::Null);
        let _ = fs::remove_dir_all(&machine.root);
    }

    #[test]
    fn the_bmc_is_named_when_there_is_one_and_left_out_when_there_is_not() {
        let machine = Machine::new("bmc");
        machine.optiplex();
        let bare = gather(&machine.sources(), &i5(), None);
        assert!(json_of(&bare).get("bmc").is_none());
        assert!(!bare.text().contains("BMC"));
        let mut sim = ipmi::sim::Sim::new();
        let bmc = bmc_of(&mut sim).unwrap();
        let show = gather(&machine.sources(), &i5(), None).with_bmc(Some(bmc));
        assert_eq!(
            json_of(&show)["bmc"],
            json!({ "maker": "Dell", "firmware": "3.20", "ipmi_version": "2.0",
                "lan_address": "10.1.2.3" })
        );
        assert!(
            show.text()
                .ends_with("BMC        Dell, firmware 3.20, IPMI 2.0\nBMC LAN    10.1.2.3\n"),
            "{}",
            show.text()
        );
        let _ = fs::remove_dir_all(&machine.root);
    }

    #[test]
    fn sizes_and_clocks_are_said_as_a_person_says_them() {
        assert_eq!(amount(16 << 30), "16.0 GiB");
        assert_eq!(amount(512 << 20), "512 MiB");
        assert_eq!(clock(3200), "3.2 GHz");
        assert_eq!(clock(4000), "4 GHz");
        assert_eq!(clock(2650), "2.65 GHz");
        assert_eq!(clock(800), "0.8 GHz");
        assert_eq!(clock(10_000), "10 GHz");
    }
}
