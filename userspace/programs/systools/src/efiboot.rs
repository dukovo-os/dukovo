// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! efiboot(8), and efibootmgr (ADR 0313): the entries of the firmware's
//! boot manager, listed, made, deleted and put in order, with the options
//! of Linux's efibootmgr, by whose name it runs too.
//!
//! The entries are UEFI variables of the global vendor (UEFI 2.10,
//! section 3.3): `Boot####`, each a load option naming a description and
//! the device path of what to start; `BootOrder`, the numbers in the order
//! the firmware tries them; `BootNext`, one to try first at the next start
//! alone; `BootCurrent`, the one it started this time; and `Timeout`. The
//! kernel reads and writes them for a program with the platform capability
//! in SLOT_PLATFORM (radix-platform(2)), which an administrator's programs
//! have, and it writes no other variables.
//!
//! A new entry names a partition by its place on the disk and its GPT GUID
//! (or the MBR's disk signature), then the file to start on it:
//! `HD(1,GPT,<guid>,0x800,0x100000)/File(\EFI\BOOT\BOOTX64.EFI)`. The
//! firmware finds the partition by those wherever the disk is attached,
//! as it does for entries Windows and Linux make.

use std::ffi::OsString;
use std::fmt::Write as _;

use radix_abi::Status;
use radix_abi::errors::{File as FileError, Kernel};
use radix_abi::platform::{self, EfiAttributes};
use radix_abi::startup::SLOT_PLATFORM;

use crate::args::{Arg, Args};
use crate::blockdev;
use crate::sys::SYS;
use crate::util::{self, Fail};

/// The attributes the boot manager's variables have: kept across
/// restarts, and readable before and after the boot services end.
const ATTRIBUTES: EfiAttributes = EfiAttributes(
    EfiAttributes::NON_VOLATILE.bits()
        | EfiAttributes::BOOT_SERVICE_ACCESS.bits()
        | EfiAttributes::RUNTIME_ACCESS.bits(),
);

/// A load option's attribute: the firmware may start it.
const LOAD_OPTION_ACTIVE: u32 = 1;

/// The loader a new entry starts if `--loader` does not say: GRUB, where
/// rgrub-setup(8) puts it on an EFI system partition.
const DEFAULT_LOADER: &str = r"\EFI\BOOT\BOOTX64.EFI";

/// The firmware's variables of the global vendor: the kernel's, or a
/// test's.
pub trait Variables {
    /// The variable `name`, or `None` if there is none.
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, Status>;
    /// Writes `name` with the boot manager's attributes.
    fn set(&mut self, name: &str, data: &[u8]) -> Result<(), Status>;
    /// Deletes `name`; deleting one there is not is no error.
    fn delete(&mut self, name: &str) -> Result<(), Status>;
    /// The names of every variable of the global vendor.
    fn names(&self) -> Result<Vec<String>, Status>;
}

/// The firmware's variables, through the kernel.
struct Firmware;

impl Variables for Firmware {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, Status> {
        let name = ucs2(name);
        let mut data = vec![0; 1024];
        loop {
            let read = platform::efi_variable(
                &SYS,
                SLOT_PLATFORM,
                platform::GLOBAL_VARIABLE_LOW,
                platform::GLOBAL_VARIABLE_HIGH,
                &name,
                &mut data,
            );
            match read {
                Ok(read) if read.length as usize <= data.len() => {
                    data.truncate(read.length as usize);
                    return Ok(Some(data));
                }
                // Longer than the buffer: nothing was copied; again with
                // room enough, which the kernel caps.
                Ok(read) if read.length <= u64::from(platform::EFI_VARIABLE_MAX) => {
                    data.resize(read.length as usize, 0);
                }
                Ok(_) => return Err(FileError::TooBig.status(0)),
                Err(status) if FileError::from_status(status) == Some(FileError::NotFound) => {
                    return Ok(None);
                }
                Err(status) => return Err(status),
            }
        }
    }

    fn set(&mut self, name: &str, data: &[u8]) -> Result<(), Status> {
        platform::set_efi_variable(
            &SYS,
            SLOT_PLATFORM,
            platform::GLOBAL_VARIABLE_LOW,
            platform::GLOBAL_VARIABLE_HIGH,
            &ucs2(name),
            ATTRIBUTES,
            data,
        )
    }

    fn delete(&mut self, name: &str) -> Result<(), Status> {
        match self.set(name, &[]) {
            Err(status) if FileError::from_status(status) == Some(FileError::NotFound) => Ok(()),
            done => done,
        }
    }

    fn names(&self) -> Result<Vec<String>, Status> {
        let mut names = Vec::new();
        // The walk goes through every vendor's variables, each named after
        // the last, its vendor and all.
        let (mut low, mut high, mut last) = (0, 0, Vec::new());
        let mut next = vec![0; 256];
        loop {
            let found =
                platform::next_efi_variable(&SYS, SLOT_PLATFORM, low, high, &last, &mut next);
            match found {
                Ok(found) if found.length as usize > next.len() => {
                    next.resize(found.length as usize, 0);
                }
                Ok(found) => {
                    last = next[..found.length as usize].to_vec();
                    (low, high) = (found.vendor_low, found.vendor_high);
                    if (low, high)
                        == (platform::GLOBAL_VARIABLE_LOW, platform::GLOBAL_VARIABLE_HIGH)
                    {
                        names.push(from_ucs2(&last));
                    }
                }
                Err(status) if FileError::from_status(status) == Some(FileError::NotFound) => {
                    return Ok(names);
                }
                Err(status) => return Err(status),
            }
        }
    }
}

/// `text` as UCS-2, little-endian, without a NUL: as the kernel takes
/// names. Characters past UCS-2 become U+FFFD.
fn ucs2(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| u16::try_from(u32::from(c)).unwrap_or(0xfffd))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// UCS-2 bytes as text, up to a NUL if there is one.
fn from_ucs2(bytes: &[u8]) -> String {
    let units = bytes.as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit));
    char::decode_utf16(units.take_while(|&unit| unit != 0))
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// What efiboot is asked to change, before it lists the entries.
#[derive(Default)]
struct Asked {
    /// Make an entry (`--create`), and put it first in the order unless
    /// `--create-only`.
    create: Option<bool>,
    /// The disk of a new entry's partition.
    disk: Option<OsString>,
    /// Its partition's number.
    part: Option<u32>,
    /// The file it starts.
    loader: Option<String>,
    /// Its description.
    label: Option<String>,
    /// The entry the other options are about (`--bootnum`).
    number: Option<u16>,
    /// Delete that entry (`--delete-bootnum`).
    delete: bool,
    /// Make that entry active or inactive.
    active: Option<bool>,
    /// A new order (`--bootorder`), or none to delete it.
    order: Option<Option<Vec<u16>>>,
    /// An entry to start next time alone, or none to delete it.
    next: Option<Option<u16>>,
    /// The firmware's menu's time to wait in seconds, or none to delete it.
    timeout: Option<Option<u16>>,
    /// Show each entry's device path.
    verbose: bool,
    /// Print nothing after a change.
    quiet: bool,
}

/// efiboot(8), and efibootmgr.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    match options(args)? {
        Some(asked) => run(&mut Firmware, &asked, &mut std::io::stdout().lock()),
        // It printed its help or version.
        None => Ok(0),
    }
}

/// Reads the options, as efibootmgr takes them; `None` once `--help` or
/// `--version` has printed what it asks.
fn options(args: &mut Args) -> Result<Option<Asked>, Fail> {
    let mut asked = Asked::default();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('c') => asked.create = Some(true),
            Arg::Long(l) if l == "create" => asked.create = Some(true),
            Arg::Short('C') => asked.create = Some(false),
            Arg::Long(l) if l == "create-only" => asked.create = Some(false),
            Arg::Short('d') => asked.disk = Some(args.value()?),
            Arg::Long(l) if l == "disk" => asked.disk = Some(args.value()?),
            Arg::Short('p') => asked.part = Some(args.number()?),
            Arg::Long(l) if l == "part" => asked.part = Some(args.number()?),
            Arg::Short('l') => asked.loader = Some(args.text()?),
            Arg::Long(l) if l == "loader" => asked.loader = Some(args.text()?),
            Arg::Short('L') => asked.label = Some(args.text()?),
            Arg::Long(l) if l == "label" => asked.label = Some(args.text()?),
            Arg::Short('b') => asked.number = Some(number(&args.text()?)?),
            Arg::Long(l) if l == "bootnum" => asked.number = Some(number(&args.text()?)?),
            Arg::Short('B') => asked.delete = true,
            Arg::Long(l) if l == "delete-bootnum" => asked.delete = true,
            Arg::Short('a') => asked.active = Some(true),
            Arg::Long(l) if l == "active" => asked.active = Some(true),
            Arg::Short('A') => asked.active = Some(false),
            Arg::Long(l) if l == "inactive" => asked.active = Some(false),
            Arg::Short('o') => asked.order = Some(Some(order(&args.text()?)?)),
            Arg::Long(l) if l == "bootorder" => asked.order = Some(Some(order(&args.text()?)?)),
            Arg::Short('O') => asked.order = Some(None),
            Arg::Long(l) if l == "delete-bootorder" => asked.order = Some(None),
            Arg::Short('n') => asked.next = Some(Some(number(&args.text()?)?)),
            Arg::Long(l) if l == "bootnext" => asked.next = Some(Some(number(&args.text()?)?)),
            Arg::Short('N') => asked.next = Some(None),
            Arg::Long(l) if l == "delete-bootnext" => asked.next = Some(None),
            Arg::Short('t') => asked.timeout = Some(Some(args.number()?)),
            Arg::Long(l) if l == "timeout" => asked.timeout = Some(Some(args.number()?)),
            Arg::Short('T') => asked.timeout = Some(None),
            Arg::Long(l) if l == "delete-timeout" => asked.timeout = Some(None),
            Arg::Short('v') => asked.verbose = true,
            Arg::Long(l) if l == "verbose" => asked.verbose = true,
            Arg::Short('q') => asked.quiet = true,
            Arg::Long(l) if l == "quiet" => asked.quiet = true,
            Arg::Short('V') => return Ok(print(&version())),
            Arg::Long(l) if l == "version" => return Ok(print(&version())),
            Arg::Short('h') => return Ok(print(USAGE)),
            Arg::Long(l) if l == "help" => return Ok(print(USAGE)),
            other => return Err(args.unknown(&other)),
        }
    }
    let about_one = asked.delete || asked.active.is_some();
    if about_one && asked.number.is_none() {
        return Err(util::usage("say which entry with --bootnum".into()));
    }
    if asked.create.is_some() && about_one {
        return Err(util::usage(
            "--create makes an entry; it does not delete or change one".into(),
        ));
    }
    Ok(Some(asked))
}

/// Prints `text`, for `--help` and `--version`.
fn print(text: &str) -> Option<Asked> {
    println!("{text}");
    None
}

const USAGE: &str = "usage: efiboot [-v] [-q] [-c | -C] [-d DISK] [-p PART] [-l LOADER] \
                     [-L LABEL] [-b XXXX] [-B] [-a | -A]\n               \
                     [-o XXXX,YYYY,... | -O] [-n XXXX | -N] [-t SECONDS | -T]";

fn version() -> String {
    format!("efiboot, Dukovo systools {}", env!("CARGO_PKG_VERSION"))
}

/// An entry's number, as `Boot####` has it: four hexadecimal digits at
/// most.
fn number(text: &str) -> Result<u16, Fail> {
    let digits = text.trim();
    if digits.is_empty() || digits.len() > 4 {
        return Err(util::usage(format!("'{text}' is not a boot entry's number, such as 0003")));
    }
    u16::from_str_radix(digits, 16)
        .map_err(|_| util::usage(format!("'{text}' is not a boot entry's number, such as 0003")))
}

/// A boot order: numbers between commas.
fn order(text: &str) -> Result<Vec<u16>, Fail> {
    let order: Vec<u16> = text.split(',').map(number).collect::<Result<_, _>>()?;
    for (at, entry) in order.iter().enumerate() {
        if order[..at].contains(entry) {
            return Err(util::usage(format!("Boot{entry:04X} is in the order twice")));
        }
    }
    Ok(order)
}

/// Does what `asked` says to `variables`, then lists the entries on
/// `out` (unless a change asked for quiet).
fn run(
    variables: &mut dyn Variables,
    asked: &Asked,
    out: &mut dyn std::io::Write,
) -> Result<i32, Fail> {
    let entries = boot_entries(variables).map_err(refused)?;
    let exists = |number: u16| entries.contains(&number);
    let mut changed = false;

    if let Some(first) = asked.create {
        let disk = asked.disk.clone().unwrap_or_else(|| "sda".into());
        let path = partition_path(&disk, asked.part.unwrap_or(1), asked.loader.as_deref())?;
        let label = asked.label.as_deref().unwrap_or("Dukovo");
        let number = match asked.number {
            Some(number) if exists(number) => {
                return Err(util::fail(format!("Boot{number:04X} is there already")));
            }
            Some(number) => number,
            None => (0..=u16::MAX)
                .find(|&number| !exists(number))
                .ok_or_else(|| util::fail("every boot entry's number is taken".into()))?,
        };
        let option = LoadOption {
            attributes: LOAD_OPTION_ACTIVE,
            description: label.to_string(),
            path,
            optional: Vec::new(),
        };
        variables.set(&boot_name(number), &option.encode()).map_err(refused)?;
        if first {
            let mut order = boot_order(variables).map_err(refused)?;
            order.retain(|&entry| entry != number);
            order.insert(0, number);
            variables.set("BootOrder", &numbers(&order)).map_err(refused)?;
        }
        changed = true;
    }

    if let Some(number) = asked.number.filter(|_| asked.create.is_none()) {
        if !exists(number) {
            return Err(util::fail(format!("there is no Boot{number:04X}")));
        }
        if asked.delete {
            variables.delete(&boot_name(number)).map_err(refused)?;
            let mut order = boot_order(variables).map_err(refused)?;
            if order.contains(&number) {
                order.retain(|&entry| entry != number);
                variables.set("BootOrder", &numbers(&order)).map_err(refused)?;
            }
            changed = true;
        }
        if let Some(active) = asked.active {
            let name = boot_name(number);
            let bytes = variables.get(&name).map_err(refused)?.unwrap_or_default();
            let mut option = LoadOption::parse(&bytes)
                .ok_or_else(|| util::fail(format!("{name} is not a load option")))?;
            option.attributes = if active {
                option.attributes | LOAD_OPTION_ACTIVE
            } else {
                option.attributes & !LOAD_OPTION_ACTIVE
            };
            variables.set(&name, &option.encode()).map_err(refused)?;
            changed = true;
        }
    }

    if let Some(order) = &asked.order {
        match order {
            Some(order) => {
                if let Some(missing) = order.iter().find(|&&entry| !exists(entry)) {
                    return Err(util::fail(format!("there is no Boot{missing:04X} to order")));
                }
                variables.set("BootOrder", &numbers(order)).map_err(refused)?;
            }
            None => variables.delete("BootOrder").map_err(refused)?,
        }
        changed = true;
    }
    if let Some(next) = asked.next {
        match next {
            Some(number) if !exists(number) => {
                return Err(util::fail(format!("there is no Boot{number:04X} to start next")));
            }
            Some(number) => variables.set("BootNext", &number.to_le_bytes()).map_err(refused)?,
            None => variables.delete("BootNext").map_err(refused)?,
        }
        changed = true;
    }
    if let Some(timeout) = asked.timeout {
        match timeout {
            Some(seconds) => variables.set("Timeout", &seconds.to_le_bytes()).map_err(refused)?,
            None => variables.delete("Timeout").map_err(refused)?,
        }
        changed = true;
    }

    if !(changed && asked.quiet) {
        let listing = list(variables, asked.verbose).map_err(refused)?;
        out.write_all(listing.as_bytes()).map_err(|e| util::failed(&"standard output", e))?;
    }
    Ok(0)
}

/// The numbers of the `Boot####` variables there are, in order.
fn boot_entries(variables: &dyn Variables) -> Result<Vec<u16>, Status> {
    let mut entries: Vec<u16> =
        variables.names()?.iter().filter_map(|name| boot_number(name)).collect();
    entries.sort_unstable();
    Ok(entries)
}

/// The number of a variable named `Boot####`, in uppercase hexadecimal.
fn boot_number(name: &str) -> Option<u16> {
    let digits = name.strip_prefix("Boot")?;
    let hex = digits.len() == 4 && digits.bytes().all(|b| matches!(b, b'0'..=b'9' | b'A'..=b'F'));
    if hex { u16::from_str_radix(digits, 16).ok() } else { None }
}

/// The variable of entry `number`.
fn boot_name(number: u16) -> String {
    format!("Boot{number:04X}")
}

/// `BootOrder`, empty if there is none.
fn boot_order(variables: &dyn Variables) -> Result<Vec<u16>, Status> {
    Ok(variables.get("BootOrder")?.map_or_else(Vec::new, |bytes| words(&bytes)))
}

/// 16-bit little-endian numbers, as `BootOrder` holds them.
fn words(bytes: &[u8]) -> Vec<u16> {
    bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect()
}

/// And back.
fn numbers(order: &[u16]) -> Vec<u8> {
    order.iter().flat_map(|number| number.to_le_bytes()).collect()
}

/// The entries as efibootmgr lists them: `BootCurrent`, `BootNext`,
/// `Timeout` and `BootOrder` where there are, then each `Boot####`, with
/// an asterisk if active, and with its device path if `verbose`.
fn list(variables: &dyn Variables, verbose: bool) -> Result<String, Status> {
    let mut text = String::new();
    let one = |name: &str| -> Result<Option<u16>, Status> {
        Ok(variables.get(name)?.and_then(|bytes| words(&bytes).first().copied()))
    };
    if let Some(current) = one("BootCurrent")? {
        let _ = writeln!(text, "BootCurrent: {current:04X}");
    }
    if let Some(next) = one("BootNext")? {
        let _ = writeln!(text, "BootNext: {next:04X}");
    }
    if let Some(timeout) = one("Timeout")? {
        let unit = if timeout == 1 { "second" } else { "seconds" };
        let _ = writeln!(text, "Timeout: {timeout} {unit}");
    }
    if let Some(bytes) = variables.get("BootOrder")? {
        let order: Vec<String> = words(&bytes).iter().map(|n| format!("{n:04X}")).collect();
        let _ = writeln!(text, "BootOrder: {}", order.join(","));
    }
    for number in boot_entries(variables)? {
        let name = boot_name(number);
        let Some(bytes) = variables.get(&name)? else { continue };
        match LoadOption::parse(&bytes) {
            Some(option) => {
                let active = if option.attributes & LOAD_OPTION_ACTIVE != 0 { "*" } else { " " };
                let _ = write!(text, "{name}{active} {}", option.description);
                if verbose {
                    let _ = write!(text, "\t{}", path_text(&option.path));
                }
                text.push('\n');
            }
            None => {
                let _ = writeln!(text, "{name}  (not a load option: {} bytes)", bytes.len());
            }
        }
    }
    Ok(text)
}

/// What to say when the kernel refused.
fn refused(status: Status) -> Fail {
    let why = match (Kernel::from_status(status), FileError::from_status(status)) {
        (Some(Kernel::NoSuchSlot | Kernel::EmptySlot | Kernel::WrongKind), _) => {
            "the firmware's variables need an administrator: run it with sudo".to_string()
        }
        (Some(Kernel::MissingRights), _) => {
            "its platform capability does not allow this: changes need the control right"
                .to_string()
        }
        (Some(Kernel::NotSupported), _) => "the firmware's variables are out of reach: the \
                                            machine started without UEFI, or the kernel log \
                                            says why"
            .to_string(),
        (Some(Kernel::NotPermitted), _) => {
            "refused: the kernel writes only the boot manager's variables, and the firmware's \
             Secure Boot rules may forbid a write"
                .to_string()
        }
        (_, Some(FileError::NoSpace)) => "the firmware has too little room left for its \
                                          variables; deleting old entries makes some"
            .to_string(),
        (_, Some(FileError::ReadOnly)) => "the firmware will not write that variable".to_string(),
        (_, Some(FileError::Io)) => {
            "the firmware failed to read or write its variables (the kernel log may say why)"
                .to_string()
        }
        _ => format!("the kernel refused ({:#x})", status.0),
    };
    util::fail(why)
}

/// A boot entry: a load option (UEFI 2.10, section 3.1.3).
#[derive(Clone, Debug, PartialEq, Eq)]
struct LoadOption {
    /// `LOAD_OPTION_ACTIVE` and the rest.
    attributes: u32,
    /// What the firmware's menu shows.
    description: String,
    /// The device path of what it starts, its end node and all.
    path: Vec<u8>,
    /// Bytes the loader gets.
    optional: Vec<u8>,
}

impl LoadOption {
    /// The load option `bytes` hold, if well formed.
    fn parse(bytes: &[u8]) -> Option<LoadOption> {
        let attributes = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
        let path_length = usize::from(u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?));
        // The description is UCS-2 up to its NUL, on 2-byte steps.
        let rest = bytes.get(6..)?;
        let end = rest.as_chunks::<2>().0.iter().position(|unit| *unit == [0, 0])?;
        let description = from_ucs2(&rest[..2 * end]);
        let path_start = 6 + 2 * (end + 1);
        let path = bytes.get(path_start..path_start.checked_add(path_length)?)?.to_vec();
        let optional = bytes[path_start + path_length..].to_vec();
        Some(LoadOption { attributes, description, path, optional })
    }

    /// Its bytes, as the variable holds them.
    fn encode(&self) -> Vec<u8> {
        let mut bytes = self.attributes.to_le_bytes().to_vec();
        bytes.extend_from_slice(&u16::try_from(self.path.len()).unwrap_or(0).to_le_bytes());
        bytes.extend(ucs2(&self.description));
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&self.path);
        bytes.extend_from_slice(&self.optional);
        bytes
    }
}

/// Device path node types (UEFI 2.10, section 10.3).
mod node {
    pub const HARDWARE: u8 = 1;
    pub const ACPI: u8 = 2;
    pub const MESSAGING: u8 = 3;
    pub const MEDIA: u8 = 4;
    pub const BBS: u8 = 5;
    pub const END: u8 = 0x7f;
    /// The end of the whole path; 0x01 ends one instance of several.
    pub const END_ENTIRE: u8 = 0xff;
    pub const HARD_DRIVE: u8 = 1;
    pub const FILE_PATH: u8 = 4;
}

/// The device path for `loader` on partition `part` of `disk`: the
/// partition by its number, place and GUID or the disk's signature, then
/// the file.
fn partition_path(disk: &OsString, part: u32, loader: Option<&str>) -> Result<Vec<u8>, Fail> {
    let opened = blockdev::open(disk, false, None)?;
    let device = opened.device().map_err(|e| util::failed(&opened.shown, e))?;
    let found = disk::read(&device).map_err(|e| util::fail(format!("{}: {e}", opened.shown)))?;
    let table = found
        .table
        .ok_or_else(|| util::fail(format!("{} has no partition table", opened.shown)))?;
    let partition = table
        .partitions
        .iter()
        .find(|p| p.number == part)
        .ok_or_else(|| util::fail(format!("{} has no partition {part}", opened.shown)))?;
    let signature = match table.scheme {
        disk::Scheme::Gpt => Signature::Gpt(partition.guid.0),
        disk::Scheme::Mbr => Signature::Mbr(table.signature),
    };
    let loader = loader_path(loader.unwrap_or(DEFAULT_LOADER));
    Ok(hard_drive_path(part, partition.first, partition.sectors(), signature, &loader))
}

/// How a hard drive node names its partition's disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signature {
    /// A GPT partition's own GUID, in its on-disk order.
    Gpt([u8; 16]),
    /// An MBR's disk signature.
    Mbr(u32),
}

/// A loader's path as the firmware takes it: backslashes, from the
/// partition's root.
fn loader_path(loader: &str) -> String {
    let path = loader.replace('/', "\\");
    if path.starts_with('\\') { path } else { format!("\\{path}") }
}

/// `HD(part,…)/File(loader)` and the end node.
fn hard_drive_path(
    part: u32,
    first: u64,
    sectors: u64,
    signature: Signature,
    loader: &str,
) -> Vec<u8> {
    let mut path = vec![node::MEDIA, node::HARD_DRIVE, 42, 0];
    path.extend_from_slice(&part.to_le_bytes());
    path.extend_from_slice(&first.to_le_bytes());
    path.extend_from_slice(&sectors.to_le_bytes());
    let (bytes, format, kind) = match signature {
        Signature::Gpt(guid) => (guid, 2, 2),
        Signature::Mbr(signature) => {
            let mut bytes = [0; 16];
            bytes[..4].copy_from_slice(&signature.to_le_bytes());
            (bytes, 1, 1)
        }
    };
    path.extend_from_slice(&bytes);
    path.extend_from_slice(&[format, kind]);
    let name: Vec<u8> = ucs2(loader).into_iter().chain([0, 0]).collect();
    let length = u16::try_from(4 + name.len()).unwrap_or(u16::MAX);
    path.extend_from_slice(&[node::MEDIA, node::FILE_PATH]);
    path.extend_from_slice(&length.to_le_bytes());
    path.extend(name);
    path.extend_from_slice(&[node::END, node::END_ENTIRE, 4, 0]);
    path
}

/// A device path as text, as efibootmgr -v shows it: the nodes it knows
/// by their names in UEFI's text form (UEFI 2.10, section 10.6), others
/// as `Path(type,subtype,bytes)`.
fn path_text(path: &[u8]) -> String {
    let mut text = String::new();
    let mut rest = path;
    while rest.len() >= 4 {
        let (kind, subtype) = (rest[0], rest[1]);
        let length = usize::from(u16::from_le_bytes([rest[2], rest[3]]));
        if length < 4 || length > rest.len() {
            text.push_str("<malformed>");
            break;
        }
        let data = &rest[4..length];
        rest = &rest[length..];
        match (kind, subtype) {
            (node::END, node::END_ENTIRE) => break,
            (node::END, _) => {
                text.push(',');
                continue;
            }
            _ => {}
        }
        if !text.is_empty() && !text.ends_with(',') {
            text.push('/');
        }
        text.push_str(&node_text(kind, subtype, data));
    }
    text
}

/// One node as text.
fn node_text(kind: u8, subtype: u8, data: &[u8]) -> String {
    let u8_at = |at: usize| data.get(at).copied().unwrap_or(0);
    let u16_at = |at: usize| data.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| {
        data.get(at..at + 4).and_then(|b| b.try_into().ok()).map_or(0, u32::from_le_bytes)
    };
    let u64_at = |at: usize| {
        data.get(at..at + 8).and_then(|b| b.try_into().ok()).map_or(0, u64::from_le_bytes)
    };
    let guid_at = |at: usize| {
        data.get(at..at + 16)
            .and_then(|b| <[u8; 16]>::try_from(b).ok())
            .map_or_else(|| "?".into(), |b| disk::Guid(b).lower())
    };
    match (kind, subtype) {
        (node::HARDWARE, 1) if data.len() >= 2 => format!("Pci({:#x},{:#x})", u8_at(1), u8_at(0)),
        (node::HARDWARE, 4) if data.len() >= 16 => format!("VenHw({})", guid_at(0)),
        (node::ACPI, 1) if data.len() >= 8 => match u32_at(0) {
            0x0a03_41d0 => format!("PciRoot({:#x})", u32_at(4)),
            0x0a08_41d0 => format!("PcieRoot({:#x})", u32_at(4)),
            hid => format!("Acpi({}{:04X},{:#x})", pnp_vendor(hid), hid >> 16, u32_at(4)),
        },
        (node::MESSAGING, 1) if data.len() >= 4 => {
            let side = if u8_at(0) == 0 { "Primary" } else { "Secondary" };
            let device = if u8_at(1) == 0 { "Master" } else { "Slave" };
            format!("Ata({side},{device},{:#x})", u16_at(2))
        }
        (node::MESSAGING, 2) if data.len() >= 4 => {
            format!("Scsi({:#x},{:#x})", u16_at(0), u16_at(2))
        }
        (node::MESSAGING, 5) if data.len() >= 2 => format!("USB({:#x},{:#x})", u8_at(0), u8_at(1)),
        (node::MESSAGING, 10) if data.len() >= 16 => format!("VenMsg({})", guid_at(0)),
        (node::MESSAGING, 11) if data.len() >= 33 => {
            format!("MAC({},{:#x})", hex(&data[..6]), u8_at(32))
        }
        (node::MESSAGING, 12) if data.len() >= 15 => {
            let ip = &data[4..8];
            format!("IPv4({}.{}.{}.{})", ip[0], ip[1], ip[2], ip[3])
        }
        (node::MESSAGING, 13) => "IPv6()".to_string(),
        (node::MESSAGING, 18) if data.len() >= 6 => {
            format!("Sata({:#x},{:#x},{:#x})", u16_at(0), u16_at(2), u16_at(4))
        }
        (node::MESSAGING, 23) if data.len() >= 12 => {
            let eui: Vec<String> = data[4..12].iter().map(|b| format!("{b:02X}")).collect();
            format!("NVMe({:#x},{})", u32_at(0), eui.join("-"))
        }
        (node::MESSAGING, 24) => format!("Uri({})", String::from_utf8_lossy(data)),
        (node::MEDIA, node::HARD_DRIVE) if data.len() >= 38 => {
            let signature = match (u8_at(36), u8_at(37)) {
                (2, 2) => format!("GPT,{}", guid_at(20)),
                (1, 1) => format!("MBR,{:#x}", u32_at(20)),
                (format, kind) => format!("{format},{kind}"),
            };
            format!("HD({},{signature},{:#x},{:#x})", u32_at(0), u64_at(4), u64_at(12))
        }
        (node::MEDIA, 2) if data.len() >= 20 => {
            format!("CDROM({:#x},{:#x},{:#x})", u32_at(0), u64_at(4), u64_at(12))
        }
        (node::MEDIA, 3) if data.len() >= 16 => format!("VenMedia({})", guid_at(0)),
        (node::MEDIA, node::FILE_PATH) => format!("File({})", from_ucs2(data)),
        (node::MEDIA, 6) if data.len() >= 16 => format!("FvFile({})", guid_at(0)),
        (node::MEDIA, 7) if data.len() >= 16 => format!("Fv({})", guid_at(0)),
        (node::MEDIA, 8) if data.len() >= 20 => {
            format!("Offset({:#x},{:#x})", u64_at(4), u64_at(12))
        }
        (node::BBS, 1) if data.len() >= 4 => {
            let end = data[4..].iter().position(|&b| b == 0).map_or(data.len(), |at| 4 + at);
            format!("BBS({:#x},{})", u16_at(0), String::from_utf8_lossy(&data[4..end]))
        }
        _ => {
            format!("Path({kind},{subtype},{})", hex(data))
        }
    }
}

/// `bytes` in lowercase hexadecimal, two digits each.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// The three letters of a compressed EISA ID, such as `PNP`.
fn pnp_vendor(hid: u32) -> String {
    // Five bits a letter, from bit 10 down, in the ID's low 16 bits.
    let id = hid & 0xffff;
    [10, 5, 0].iter().map(|shift| char::from(b'@' + ((id >> shift) & 0x1f) as u8)).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// The variables of a test, as the firmware would keep them.
    #[derive(Default)]
    struct Fake(BTreeMap<String, Vec<u8>>);

    impl Variables for Fake {
        fn get(&self, name: &str) -> Result<Option<Vec<u8>>, Status> {
            Ok(self.0.get(name).cloned())
        }

        fn set(&mut self, name: &str, data: &[u8]) -> Result<(), Status> {
            self.0.insert(name.to_string(), data.to_vec());
            Ok(())
        }

        fn delete(&mut self, name: &str) -> Result<(), Status> {
            self.0.remove(name);
            Ok(())
        }

        fn names(&self) -> Result<Vec<String>, Status> {
            Ok(self.0.keys().cloned().collect())
        }
    }

    /// OVMF's entries as they are on a new machine: the UEFI shell's menu
    /// and a disk.
    fn ovmf() -> Fake {
        let mut fake = Fake::default();
        let menu = LoadOption {
            attributes: LOAD_OPTION_ACTIVE,
            description: "UiApp".into(),
            path: vec![
                4, 6, 20, 0, 0xc9, 0xbd, 0xb8, 0x7c, 0xeb, 0xf8, 0x34, 0x4f, 0xaa, 0xea, 0x3e,
                0xe4, 0xaf, 0x65, 0x16, 0xa1, 0x7f, 0xff, 4, 0,
            ],
            optional: Vec::new(),
        };
        let mut disk = vec![2, 1, 12, 0, 0xd0, 0x41, 0x03, 0x0a, 0, 0, 0, 0];
        disk.extend_from_slice(&[1, 1, 6, 0, 0, 0x1f, 0x7f, 0xff, 4, 0]);
        let disk = LoadOption {
            attributes: LOAD_OPTION_ACTIVE,
            description: "UEFI Misc Device".into(),
            path: disk,
            optional: b"N\xac\x08\x81".to_vec(),
        };
        fake.0.insert("Boot0000".into(), menu.encode());
        fake.0.insert("Boot0001".into(), disk.encode());
        fake.0.insert("BootOrder".into(), numbers(&[1, 0]));
        fake.0.insert("BootCurrent".into(), numbers(&[1]));
        fake.0.insert("Timeout".into(), numbers(&[0]));
        // Another variable of the global vendor, which is no entry.
        fake.0.insert("PlatformLang".into(), b"en-US\0".to_vec());
        fake
    }

    fn listing(fake: &mut Fake, asked: &Asked) -> String {
        let mut out = Vec::new();
        run(fake, asked, &mut out).unwrap_or_else(|fail| panic!("{}", fail.message));
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn lists_as_efibootmgr_does() {
        let mut fake = ovmf();
        let plain = listing(&mut fake, &Asked::default());
        assert_eq!(
            plain,
            "BootCurrent: 0001\nTimeout: 0 seconds\nBootOrder: 0001,0000\nBoot0000* UiApp\n\
             Boot0001* UEFI Misc Device\n"
        );
        let verbose = listing(&mut fake, &Asked { verbose: true, ..Asked::default() });
        assert!(
            verbose.contains("Boot0000* UiApp\tFvFile(7cb8bdc9-f8eb-4f34-aaea-3ee4af6516a1)\n")
        );
        assert!(verbose.contains("Boot0001* UEFI Misc Device\tPciRoot(0x0)/Pci(0x1f,0x0)\n"));
    }

    #[test]
    fn load_options_read_back_as_written() {
        let option = LoadOption {
            attributes: LOAD_OPTION_ACTIVE,
            description: "Dukovo".into(),
            path: hard_drive_path(1, 2048, 1 << 20, Signature::Gpt([7; 16]), DEFAULT_LOADER),
            optional: b"extra".to_vec(),
        };
        let bytes = option.encode();
        assert_eq!(&bytes[..4], &[1, 0, 0, 0]);
        assert_eq!(LoadOption::parse(&bytes), Some(option));
        // Cut short: no NUL after the description, or the path cut.
        assert_eq!(LoadOption::parse(&bytes[..10]), None);
        assert_eq!(LoadOption::parse(&bytes[..30]), None);
    }

    #[test]
    fn hard_drive_paths_are_as_uefi_lays_them_out() {
        let guid = disk::Guid::from_text("4C5F2A4E-0D1B-4A7E-9E3B-2C1D0E8F6A5B").unwrap();
        let path = hard_drive_path(
            1,
            0x800,
            0x10_0000,
            Signature::Gpt(guid.0),
            "\\EFI\\BOOT\\BOOTX64.EFI",
        );
        // The hard drive node is 42 bytes long, the file path node 4 more
        // than its name, NUL and all, and the end node 4.
        assert_eq!(path.len(), 42 + 4 + 2 * (21 + 1) + 4);
        assert_eq!(&path[..4], &[4, 1, 42, 0]);
        assert_eq!(&path[40..42], &[2, 2]);
        assert_eq!(
            path_text(&path),
            "HD(1,GPT,4c5f2a4e-0d1b-4a7e-9e3b-2c1d0e8f6a5b,0x800,0x100000)/\
             File(\\EFI\\BOOT\\BOOTX64.EFI)"
        );
        let mbr = hard_drive_path(2, 63, 1000, Signature::Mbr(0x1234_abcd), "\\a.efi");
        assert_eq!(path_text(&mbr), "HD(2,MBR,0x1234abcd,0x3f,0x3e8)/File(\\a.efi)");
        assert_eq!(loader_path("EFI/dukovo/grubx64.efi"), "\\EFI\\dukovo\\grubx64.efi");
    }

    #[test]
    fn odd_paths_are_shown_not_trusted() {
        assert_eq!(path_text(&[9, 9, 6, 0, 0xab, 0xcd, 0x7f, 0xff, 4, 0]), "Path(9,9,abcd)");
        // A node longer than the path.
        assert_eq!(path_text(&[4, 4, 99, 0, 0x41, 0]), "<malformed>");
        assert_eq!(
            path_text(&[2, 1, 12, 0, 0xd0, 0x41, 0x01, 0x05, 3, 0, 0, 0]),
            "Acpi(PNP0501,0x3)"
        );
    }

    #[test]
    fn deletes_reorders_and_sets_the_next_start() {
        let mut fake = ovmf();
        let asked = Asked { order: Some(Some(vec![0, 1])), quiet: true, ..Asked::default() };
        assert_eq!(listing(&mut fake, &asked), "");
        assert_eq!(words(&fake.0["BootOrder"]), [0, 1]);

        let asked = Asked { next: Some(Some(0)), timeout: Some(Some(5)), ..Asked::default() };
        let text = listing(&mut fake, &asked);
        assert!(text.contains("BootNext: 0000\nTimeout: 5 seconds\n"), "{text}");

        let asked = Asked { number: Some(1), delete: true, ..Asked::default() };
        let text = listing(&mut fake, &asked);
        assert!(!text.contains("Boot0001"), "{text}");
        assert_eq!(words(&fake.0["BootOrder"]), [0]);

        let asked = Asked { number: Some(0), active: Some(false), ..Asked::default() };
        assert!(listing(&mut fake, &asked).contains("Boot0000  UiApp\n"));

        let asked = Asked { next: Some(None), timeout: Some(None), ..Asked::default() };
        let text = listing(&mut fake, &asked);
        assert!(!text.contains("BootNext") && !text.contains("Timeout"), "{text}");
    }

    #[test]
    fn refuses_entries_there_are_not() {
        let mut fake = ovmf();
        let mut out = Vec::new();
        for asked in [
            Asked { number: Some(7), delete: true, ..Asked::default() },
            Asked { order: Some(Some(vec![1, 7])), ..Asked::default() },
            Asked { next: Some(Some(7)), ..Asked::default() },
        ] {
            assert!(run(&mut fake, &asked, &mut out).is_err());
        }
        assert_eq!(words(&fake.0["BootOrder"]), [1, 0]);
        assert!(order("0001,1").is_err());
        assert!(number("10000").is_err());
        assert_eq!(number("a").ok(), Some(10));
    }

    #[test]
    fn names_go_to_the_kernel_as_ucs2() {
        assert_eq!(ucs2("Boot0001"), b"B\0o\0o\0t\x000\x000\x000\x001\0");
        assert_eq!(from_ucs2(b"B\0o\0o\0t\0\0\0junk"), "Boot");
        assert_eq!(boot_number("Boot00A1"), Some(0xa1));
        assert_eq!(boot_number("Boot00a1"), None);
        assert_eq!(boot_number("BootOrder"), None);
    }
}
