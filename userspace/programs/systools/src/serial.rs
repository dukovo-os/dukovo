// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! serial(8): the computer's serial ports, and what its next start does
//! with them (ADR 0481). `serial list` shows each port the kernel found
//! (`/kernel/class/tty`, radix-sysfs(5)) with the console it has at this
//! start, as serial-console(8) chose it, and then the settings the next
//! start will run with. `serial console` and `serial log` change those
//! settings: which ports have a console and at what line, which carry the
//! kernel log, and so which ports GRUB's boot menu shows on.
//!
//! The kernel and serial-console read the command line, which a start
//! makes, so a change acts at the next start. It is written as the GRUB
//! environment block `boot/grub/serial.env` (`slots::serial`) on every EFI
//! system partition of the disks the system started from, under
//! sysupdate's lock, and the boot menu (image/grub/installed.cfg.in) reads
//! it from there. The installer calls `serial console ... --esp DIR` to
//! write the block on the partition it has mounted at `DIR`.
//!
//! Where to start when a start ignores what was set: `serial list` shows
//! the block as the next start reads it, and the boot tests serial-firmware
//! and install-serial cover the command.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use slots::serial::{
    Console, Consoles, Log, MENU_PORTS, MenuPort, PATH as SERIAL_ENV, Settings, grub_name,
    menu_line, speed,
};

use crate::args::{Arg, Args};
use crate::serial_console::{self, Choice, is_primary};
use crate::sysupdate::place;
use crate::util::{self, Fail, fail, usage};

/// Where the kernel lists the serial ports it found.
pub const TTY_CLASS: &str = "/kernel/class/tty";

/// What a system that did not start from a Dukovo root's disk lacks: the
/// EFI system partitions that hold the boot menu's serial settings.
const NO_ROOT: &str = "this system did not start from a Dukovo root on a disk";

/// How long a change waits for sysupdate's lock, as the health check does.
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// The options of a console given with none: the line GRUB and the
/// kernel's log run.
const DEFAULT_OPTIONS: &str = "115200";

/// A serial port the kernel found, as `/kernel/class/tty/ttyS<n>` shows it.
pub struct Port {
    /// Its name: `ttyS1`.
    pub name: String,
    /// Its first I/O port: 0x2F8.
    base: u16,
    /// Whether it carries the kernel log.
    log: bool,
    /// What the kernel calls it: `com2`, `amt-sol`, `ax99100`, `qemu`,
    /// `pci`.
    kind: String,
    /// The PCI function it is on, `0000:00:16.3`, if it is on one.
    pci: Option<String>,
    /// The options of its line, `57600n8`, when the firmware sends its
    /// console to it.
    pub firmware: Option<String>,
}

impl Port {
    /// The port `name` as the kernel lists it in `dir`, if the files the
    /// kernel always writes for a port are there.
    fn read(dir: &Path, name: &str) -> Option<Port> {
        let text = |file: &str| {
            let text = std::fs::read_to_string(dir.join(name).join(file)).ok()?;
            Some(text.trim().to_string())
        };
        let base = u16::from_str_radix(text("port")?.strip_prefix("0x")?, 16).ok()?;
        Some(Port {
            name: name.to_string(),
            base,
            log: text("console")? == "Y",
            kind: text("kind")?,
            pci: text("pci"),
            firmware: text("firmware_console"),
        })
    }

    /// Whether it is the first port of its PCI function, the one GRUB
    /// runs: no other port of the function has a lower I/O port. A port
    /// that is not on PCI is the first of a function of its own.
    fn is_first(&self, ports: &[Port]) -> bool {
        self.pci.is_none()
            || !ports.iter().any(|other| other.pci == self.pci && other.base < self.base)
    }

    /// What it is, in words: `COM2 at I/O port 0x2F8`.
    fn what(&self) -> String {
        if self.kind.starts_with("com") {
            return format!("{} at I/O port {:#X}", self.kind.to_ascii_uppercase(), self.base);
        }
        let card = match self.kind.as_str() {
            "amt-sol" => "Intel AMT Serial-over-LAN",
            "ax99100" => "ASIX AX99100 card",
            "qemu" => "QEMU PCI serial card",
            _ => "PCI serial card",
        };
        format!("{card} at PCI {}", self.pci.as_deref().unwrap_or("an unknown address"))
    }

    /// The port as GRUB's menu shows on it, at the speed of console
    /// `options`, if it can: GRUB names the port, and the options are 8
    /// data bits and no parity.
    fn menu_port(&self, ports: &[Port], options: &str) -> Option<MenuPort> {
        let grub = grub_name(&self.kind, self.pci.as_deref(), self.is_first(ports))?;
        if !menu_line(options) {
            return None;
        }
        Some(MenuPort { grub, speed: speed(options)? })
    }
}

/// The serial ports the kernel lists in `dir`, by number.
pub fn ports(dir: &Path) -> Vec<Port> {
    let mut names: Vec<(u32, String)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| Some((name.strip_prefix("ttyS")?.parse().ok()?, name)))
        .collect();
    names.sort();
    names.iter().filter_map(|(_, name)| Port::read(dir, name)).collect()
}

/// What `serial` was asked to do.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// Show the ports and the next start.
    List,
    Change(Change),
}

/// A change of the settings, as asked: its port names not yet checked
/// against the ports there are.
#[derive(Debug, PartialEq, Eq)]
enum Change {
    /// `console auto`: the firmware's console port, if it names one.
    Auto,
    /// `console off`: no console.
    Off,
    /// `console PORT[,OPTIONS]...`: each port with the options after its
    /// comma, if it has any.
    Consoles(Vec<(String, Option<String>)>),
    /// `log ...`: the ports that carry the kernel log.
    Log(Log),
}

/// serial(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let (action, esp) = parse(args)?;
    let ports = ports(Path::new(TTY_CLASS));
    let Action::Change(change) = action else {
        // The settings are on the EFI system partitions, whose devices only
        // root reads.
        let next = if util::own_user() == 0 { read_next(None) } else { Next::NotRoot };
        print!("{}", list_text(&ports, &command_line(), &next));
        return Ok(0);
    };
    require_root(util::own_user())?;
    let current = match read_next(esp.as_deref()) {
        Next::NoRoot => {
            return Err(fail(format!(
                "{NO_ROOT}, so its boot menu has no serial settings to change"
            )));
        }
        Next::Unreadable(why) => return Err(fail(why)),
        Next::Block(settings) => Some(settings),
        Next::NoBlock | Next::Edited | Next::NotRoot => None,
    };
    let settings = apply(current, &ports, &change)?;
    let problems = write(&settings, esp.as_deref())?;
    for problem in &problems {
        eprintln!("serial: {problem}");
    }
    print!("{}", settings_text(&settings, &ports));
    println!("The serial console's settings take effect at the next start.");
    Ok(i32::from(!problems.is_empty()))
}

/// The action and the folder `--esp` names, if it does.
fn parse(args: &mut Args) -> Result<(Action, Option<PathBuf>), Fail> {
    let action = args.subcommand().map(|action| action.to_string_lossy().into_owned());
    let mut operands: Vec<String> = Vec::new();
    let mut esp = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "esp" => esp = Some(PathBuf::from(args.value()?)),
            Arg::Operand(operand) => operands.push(operand.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let action = match (action.as_deref(), operands.as_slice()) {
        (None | Some("list"), []) => Action::List,
        (Some("console"), [one]) if one == "auto" => Action::Change(Change::Auto),
        (Some("console"), [one]) if one == "off" => Action::Change(Change::Off),
        (Some("console"), [_, ..]) => Action::Change(Change::Consoles(
            operands
                .iter()
                .map(|spec| match spec.split_once(',') {
                    Some((port, options)) => (port.to_string(), Some(options.to_string())),
                    None => (spec.clone(), None),
                })
                .collect(),
        )),
        (Some("log"), [one]) => Action::Change(Change::Log(match one.as_str() {
            "all" => Log::All,
            "none" => Log::None,
            names => Log::Ports(names.split(',').map(str::to_string).collect()),
        })),
        _ => {
            return Err(usage(
                "say list, console PORT[,OPTIONS]..., console auto, console off, or log all, \
                 none or PORT[,PORT]..."
                    .into(),
            ));
        }
    };
    if esp.is_some() && action == Action::List {
        return Err(usage("--esp goes with console and log".into()));
    }
    Ok((action, esp))
}

/// Fails unless `user` is root, who alone may write the EFI system
/// partitions.
fn require_root(user: u32) -> Result<(), Fail> {
    if user == 0 {
        return Ok(());
    }
    Err(fail(
        "only root may change the serial console's settings: elevate first, as `sudo serial ...`"
            .into(),
    ))
}

/// The command line the kernel was started with.
fn command_line() -> String {
    std::fs::read_to_string("/kernel/cmdline").unwrap_or_default()
}

/// The settings `change` makes of the `current` ones, or of a system's
/// with no block: the firmware's console, the log on every port, and the
/// menu as an install has always had it.
fn apply(current: Option<Settings>, ports: &[Port], change: &Change) -> Result<Settings, Fail> {
    let mut settings = current.unwrap_or_else(|| Settings::new(auto_menu(ports)));
    match change {
        Change::Auto => {
            settings.consoles = Consoles::Auto;
            settings.menu = auto_menu(ports);
        }
        Change::Off => {
            settings.consoles = Consoles::Off;
            settings.menu = Vec::new();
        }
        Change::Consoles(asked) => {
            let consoles = consoles_of(asked, ports)?;
            settings.menu = menu_of(&consoles, ports);
            settings.consoles = Consoles::Ports(consoles);
        }
        Change::Log(log) => {
            if let Log::Ports(names) = log {
                names.iter().try_for_each(|name| known(name, ports))?;
            }
            settings.log = log.clone();
        }
    }
    Ok(settings)
}

/// The consoles `asked` for, each a port there is with options Linux
/// reads, in whole.
fn consoles_of(asked: &[(String, Option<String>)], ports: &[Port]) -> Result<Vec<Console>, Fail> {
    let mut consoles: Vec<Console> = Vec::new();
    for (name, options) in asked {
        known(name, ports)?;
        if consoles.iter().any(|console| console.port == *name) {
            return Err(fail(format!("{name} is named twice")));
        }
        let options = options.as_deref().unwrap_or(DEFAULT_OPTIONS);
        serial_console::format_of(name, options).map_err(fail)?;
        consoles.push(Console { port: name.clone(), options: whole(options) });
    }
    Ok(consoles)
}

/// Fails unless `name` is one of `ports`.
fn known(name: &str, ports: &[Port]) -> Result<(), Fail> {
    if ports.iter().any(|port| port.name == name) {
        return Ok(());
    }
    let names: Vec<&str> = ports.iter().map(|port| port.name.as_str()).collect();
    let have = if names.is_empty() {
        "this computer has no serial ports".to_string()
    } else {
        format!("this computer's serial ports are {}", names.join(", "))
    };
    Err(fail(format!("no {name}: {have}")))
}

/// Console options `options`, which Linux reads, with the parity and the
/// data bits they leave out given: `57600` is `57600n8`. A final `r` stays.
fn whole(options: &str) -> String {
    let digits = options.find(|c: char| !c.is_ascii_digit()).unwrap_or(options.len());
    let (speed, rest) = options.split_at(digits);
    let mut rest = rest.chars();
    let parity = rest.next().unwrap_or('n');
    let bits = rest.next().unwrap_or('8');
    format!("{speed}{parity}{bits}{}", rest.as_str())
}

/// The ports GRUB's menu shows on when the consoles are `consoles`: each
/// console port in order that GRUB can show it on, up to the most there
/// are slots for.
fn menu_of(consoles: &[Console], ports: &[Port]) -> Vec<MenuPort> {
    consoles
        .iter()
        .filter_map(|console| {
            let port = ports.iter().find(|port| port.name == console.port)?;
            port.menu_port(ports, &console.options)
        })
        .take(MENU_PORTS)
        .collect()
}

/// The menu when the firmware's console is the console: its port at the
/// firmware's speed, if GRUB can show it there; else COM1 at 115200 baud,
/// the menu an install has always had.
fn auto_menu(ports: &[Port]) -> Vec<MenuPort> {
    let firmware = ports.iter().find_map(|port| port.menu_port(ports, port.firmware.as_deref()?));
    let com1 = || ports.iter().find(|port| port.name == "ttyS0")?.menu_port(ports, "115200");
    firmware.or_else(com1).into_iter().collect()
}

/// What the next start will use, as the boot menu's block says.
enum Next {
    /// The system did not start from a Dukovo root on a disk, and no
    /// partition was named.
    NoRoot,
    /// Not run as root, which alone reads the partitions.
    NotRoot,
    /// No partition holds a block.
    NoBlock,
    /// A block that is not as `serial` writes it.
    Edited,
    /// The partitions could not be read.
    Unreadable(String),
    /// The settings of the first partition that has a block.
    Block(Settings),
}

/// The block the next start reads: in the folder `esp` names, or on the
/// first of the root's EFI system partitions that has one.
fn read_next(esp: Option<&Path>) -> Next {
    if let Some(dir) = esp {
        return match std::fs::read(dir.join(SERIAL_ENV)) {
            Ok(bytes) => Settings::decode(&bytes).map_or(Next::Edited, Next::Block),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Next::NoBlock,
            Err(err) => Next::Unreadable(format!("{}: {err}", dir.display())),
        };
    }
    // Any reason there is no root comes to the same: no boot menu to read.
    let Ok(root) = place::root() else { return Next::NoRoot };
    let partitions = match place::partitions(&root) {
        Ok(partitions) => partitions,
        Err(why) => return Next::Unreadable(why.message),
    };
    let (mut edited, mut unreadable) = (false, None);
    for name in &partitions {
        match read_partition(name) {
            Ok(Some(bytes)) => match Settings::decode(&bytes) {
                Some(settings) => return Next::Block(settings),
                None => edited = true,
            },
            Ok(None) => {}
            Err(why) => unreadable = Some(why.message),
        }
    }
    match (edited, unreadable) {
        (true, _) => Next::Edited,
        (false, Some(why)) => Next::Unreadable(why),
        (false, None) => Next::NoBlock,
    }
}

/// The block on the EFI system partition `name`, if it has one.
fn read_partition(name: &str) -> Result<Option<Vec<u8>>, Fail> {
    disk::esp::read_file(place::device(name, false)?, SERIAL_ENV, slots::envblock::BLOCK)
        .map_err(|why| fail(format!("/devices/{name}: {why}")))
}

/// Writes `settings` as the block: in the folder `esp` names, or on each
/// of the root's EFI system partitions under sysupdate's lock, so that no
/// FAT is written while a stage or the health check writes it. The
/// partitions it could not write, with why, when some could.
///
/// # Errors
///
/// If the settings do not fit, there is no root, or no partition could be
/// written.
fn write(settings: &Settings, esp: Option<&Path>) -> Result<Vec<String>, Fail> {
    let bytes =
        settings.encode().ok_or_else(|| fail(format!("the settings do not fit {}", SERIAL_ENV)))?;
    if let Some(dir) = esp {
        let file = dir.join(SERIAL_ENV);
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder).map_err(|err| util::failed(&folder.display(), err))?;
        }
        std::fs::write(&file, bytes).map_err(|err| util::failed(&file.display(), err))?;
        return Ok(Vec::new());
    }
    let root = place::root().map_err(|_| {
        fail(format!("{NO_ROOT}, so its boot menu has no serial settings to change"))
    })?;
    let _lock = place::lock_within(LOCK_WAIT)?;
    let partitions = place::partitions(&root)?;
    if partitions.is_empty() {
        return Err(fail(format!(
            "the disks of the root {} have no EFI system partition, where the boot menu is",
            root.uuid
        )));
    }
    let mut problems = Vec::new();
    for name in &partitions {
        let written = place::device(name, true).and_then(|device| {
            disk::esp::overwrite_file(device, SERIAL_ENV, &bytes)
                .map_err(|why| fail(format!("/devices/{name}: {}: {why}", SERIAL_ENV)))
        });
        if let Err(why) = written {
            problems.push(why.message);
        }
    }
    if problems.len() == partitions.len() {
        return Err(fail(problems.join("; ")));
    }
    Ok(problems)
}

/// What `serial list` prints: the ports with the console each has at this
/// start, by command line `line`, the firmware's console port, and `next`.
fn list_text(ports: &[Port], line: &str, next: &Next) -> String {
    let mut text = if ports.is_empty() {
        String::from("This computer has no serial ports.\n")
    } else {
        table(ports, line)
    };
    if let Some(port) = ports.iter().find(|port| port.firmware.is_some()) {
        let options = port.firmware.as_deref().unwrap_or_default();
        text.push_str(&format!(
            "\nThe firmware sends its console to {} (ACPI SPCR), at {options}.\n",
            port.name
        ));
    }
    text.push('\n');
    text.push_str(&next_text(next, ports));
    text
}

/// The ports as columns: the port, what it is, whether it carries the
/// kernel log, and the console it has now.
fn table(ports: &[Port], line: &str) -> String {
    let listed: Vec<(String, Option<String>)> =
        ports.iter().map(|port| (port.name.clone(), port.firmware.clone())).collect();
    let choice = serial_console::choose(line, &listed);
    let header = ["PORT", "WHAT", "LOG", "CONSOLE NOW"].map(str::to_string);
    let rows = ports.iter().map(|port| {
        [
            port.name.clone(),
            port.what(),
            (if port.log { "yes" } else { "no" }).to_string(),
            console_now(&choice, line, &port.name),
        ]
    });
    let rows: Vec<[String; 4]> = std::iter::once(header).chain(rows).collect();
    let width = |column: usize| rows.iter().map(|row| row[column].len()).max().unwrap_or(0);
    let (port, what, log) = (width(0), width(1), width(2));
    let mut text = String::new();
    for row in &rows {
        let _ = writeln!(text, "{:port$}  {:what$}  {:log$}  {}", row[0], row[1], row[2], row[3]);
    }
    text
}

/// The console port `name` has at this start, under the choice
/// serial-console makes by command line `line`: its options, with
/// `primary, ` before the primary console's, `firmware, ` before the
/// firmware's, `any` under `serial-console=any`, or `-` for none.
fn console_now(choice: &Choice, line: &str, name: &str) -> String {
    match choice {
        Choice::Any(_) => "any".to_string(),
        Choice::Flags(asked) => {
            let Some(port) = asked.iter().find(|asked| asked.name == name) else {
                return "-".to_string();
            };
            if is_primary(line, name) {
                format!("primary, {}", port.options())
            } else {
                port.options()
            }
        }
        Choice::Firmware(port) if port.name == name => format!("firmware, {}", port.options()),
        Choice::Firmware(_) | Choice::Nothing => "-".to_string(),
    }
}

/// What the next start does, by what `next` says.
fn next_text(next: &Next, ports: &[Port]) -> String {
    match next {
        Next::NoRoot => format!("At the next start: {NO_ROOT}.\n"),
        Next::NotRoot => "At the next start: only root reads the boot menu's serial settings, \
                          as `sudo serial`.\n"
            .to_string(),
        Next::NoBlock => format!(
            "At the next start: as this one, since the boot menu has no serial settings ({}).\n",
            SERIAL_ENV
        ),
        Next::Edited => format!(
            "At the next start: the boot menu's serial settings are not as serial writes them \
             ({}), and the next change replaces them.\n",
            SERIAL_ENV
        ),
        Next::Unreadable(why) => {
            format!("At the next start: the boot menu's serial settings cannot be read: {why}\n")
        }
        Next::Block(settings) => settings_text(settings, ports),
    }
}

/// The consoles, the log and the boot menu of `settings`, and what keeps a
/// console off the menu.
fn settings_text(settings: &Settings, ports: &[Port]) -> String {
    let consoles = match &settings.consoles {
        Consoles::Auto => {
            let firmware = ports.iter().find(|port| port.firmware.is_some());
            let named = firmware.map_or_default(|port| format!(", {}", port.name));
            format!("the firmware's console port{named}")
        }
        Consoles::Off => "none".to_string(),
        Consoles::Ports(consoles) => {
            let each: Vec<String> = consoles
                .iter()
                .enumerate()
                .map(|(at, Console { port, options })| {
                    let primary = if at + 1 == consoles.len() { " (primary)" } else { "" };
                    format!("{port} at {options}{primary}")
                })
                .collect();
            each.join(", ")
        }
    };
    let log = match &settings.log {
        Log::All => "all ports".to_string(),
        Log::None => "no port".to_string(),
        Log::Ports(names) => names.join(", "),
    };
    let menu = if settings.menu.is_empty() {
        "none".to_string()
    } else {
        let each: Vec<String> =
            settings.menu.iter().map(|m| format!("{} at {} baud", m.grub, m.speed)).collect();
        each.join(", ")
    };
    let mut text = format!(
        "At the next start ({}):\n  Consoles:   {consoles}\n  Kernel log: {log}\n  Boot menu:  {menu}\n",
        SERIAL_ENV
    );
    if let Consoles::Ports(consoles) = &settings.consoles {
        for console in consoles {
            if let Some(why) = off_the_menu(console, ports, &settings.menu) {
                text.push_str(&format!(
                    "  {} has a console but not the boot menu: {why}.\n",
                    console.port
                ));
            }
        }
    }
    text
}

/// Why `console`, a port of `ports`, is not on `menu`, if it is not.
fn off_the_menu(console: &Console, ports: &[Port], menu: &[MenuPort]) -> Option<&'static str> {
    let port = ports.iter().find(|port| port.name == console.port)?;
    match grub_name(&port.kind, port.pci.as_deref(), port.is_first(ports)) {
        None => Some(match port.pci.as_deref() {
            Some(_) if !port.is_first(ports) => "GRUB shows it only on a PCI card's first port",
            Some(address) if !address.starts_with("0000:") => {
                "GRUB shows no port on a PCI domain other than 0000"
            }
            _ => "GRUB has no name for it",
        }),
        Some(_) if !menu_line(&console.options) => {
            Some("GRUB shows it only at 8 data bits and no parity")
        }
        Some(grub) if menu.iter().any(|shown| shown.grub == grub) => None,
        Some(_) => Some("GRUB shows it on four ports at most"),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn port(name: &str, base: u16, kind: &str, pci: Option<&str>) -> Port {
        Port {
            name: name.to_string(),
            base,
            log: true,
            kind: kind.to_string(),
            pci: pci.map(str::to_string),
            firmware: None,
        }
    }

    /// COM1, COM2 with the firmware's console, and AMT's port: the
    /// machine of the design's examples.
    fn machine() -> Vec<Port> {
        let mut ports = vec![
            port("ttyS0", 0x3F8, "com1", None),
            port("ttyS1", 0x2F8, "com2", None),
            port("ttyS4", 0xF0E0, "amt-sol", Some("0000:00:16.3")),
        ];
        ports[1].log = false;
        ports
    }

    fn asked(items: &[&str]) -> Result<(Action, Option<PathBuf>), Fail> {
        parse(&mut Args::new("serial", items.iter().map(OsString::from).collect()))
    }

    fn change(items: &[&str]) -> Change {
        match asked(items).unwrap().0 {
            Action::Change(change) => change,
            Action::List => panic!("{items:?} asked for the list"),
        }
    }

    #[test]
    fn the_list_is_asked_for_by_name_or_by_nothing() {
        assert_eq!(asked(&[]).unwrap(), (Action::List, None));
        assert_eq!(asked(&["list"]).unwrap(), (Action::List, None));
        assert!(asked(&["list", "ttyS0"]).is_err());
        assert!(asked(&["list", "--esp", "/mnt"]).is_err());
        assert!(asked(&["frobnicate"]).is_err());
        assert!(asked(&["--esp"]).is_err());
    }

    #[test]
    fn consoles_are_named_by_port_with_options_after_a_comma() {
        let ports =
            vec![("ttyS1".to_string(), Some("57600".to_string())), ("ttyS4".to_string(), None)];
        assert_eq!(change(&["console", "ttyS1,57600", "ttyS4"]), Change::Consoles(ports));
        assert_eq!(change(&["console", "auto"]), Change::Auto);
        assert_eq!(change(&["console", "off"]), Change::Off);
        assert!(asked(&["console"]).is_err());
        // `auto` beside a port is a port's name, which is not one there is.
        let beside = change(&["console", "auto", "ttyS1"]);
        assert_eq!(
            apply(None, &machine(), &beside).unwrap_err().message.get(..8),
            Some("no auto:")
        );
    }

    #[test]
    fn the_log_is_all_none_or_a_list_of_ports() {
        assert_eq!(change(&["log", "all"]), Change::Log(Log::All));
        assert_eq!(change(&["log", "none"]), Change::Log(Log::None));
        let names = vec!["ttyS0".to_string(), "ttyS4".to_string()];
        assert_eq!(change(&["log", "ttyS0,ttyS4"]), Change::Log(Log::Ports(names)));
        assert!(asked(&["log"]).is_err());
        assert!(asked(&["log", "ttyS0", "ttyS4"]).is_err());
    }

    #[test]
    fn the_esp_is_the_folder_after_the_option() {
        let (action, esp) = asked(&["console", "auto", "--esp", "/work/esp"]).unwrap();
        assert_eq!(action, Action::Change(Change::Auto));
        assert_eq!(esp, Some(PathBuf::from("/work/esp")));
        let (_, esp) = asked(&["log", "none", "--esp=/work/esp"]).unwrap();
        assert_eq!(esp, Some(PathBuf::from("/work/esp")));
    }

    #[test]
    fn only_root_changes_the_settings() {
        assert!(require_root(0).is_ok());
        let refused = require_root(1000).unwrap_err();
        assert!(refused.message.starts_with("only root may change the serial console's settings"));
        assert_eq!(refused.code, 1);
    }

    #[test]
    fn what_each_kind_of_port_is_says_where_it_is() {
        let ports = machine();
        assert_eq!(ports[0].what(), "COM1 at I/O port 0x3F8");
        assert_eq!(ports[1].what(), "COM2 at I/O port 0x2F8");
        assert_eq!(ports[2].what(), "Intel AMT Serial-over-LAN at PCI 0000:00:16.3");
        let what = |kind| port("ttyS5", 0xE000, kind, Some("0000:03:00.0")).what();
        assert_eq!(what("ax99100"), "ASIX AX99100 card at PCI 0000:03:00.0");
        assert_eq!(what("qemu"), "QEMU PCI serial card at PCI 0000:03:00.0");
        assert_eq!(what("pci"), "PCI serial card at PCI 0000:03:00.0");
    }

    #[test]
    fn a_port_is_first_when_no_port_of_its_function_has_a_lower_address() {
        let mut ports = machine();
        ports.push(port("ttyS5", 0xF0E8, "amt-sol", Some("0000:00:16.3")));
        ports.push(port("ttyS6", 0xE000, "pci", Some("0000:03:00.0")));
        let first =
            |name: &str| ports.iter().find(|port| port.name == name).unwrap().is_first(&ports);
        assert!(first("ttyS0"));
        assert!(first("ttyS4"));
        assert!(!first("ttyS5"));
        assert!(first("ttyS6"));
    }

    #[test]
    fn the_options_are_made_whole() {
        assert_eq!(whole("57600"), "57600n8");
        assert_eq!(whole("57600n"), "57600n8");
        assert_eq!(whole("9600e7"), "9600e7");
        assert_eq!(whole("115200n8r"), "115200n8r");
        assert_eq!(whole("9600o8r"), "9600o8r");
    }

    #[test]
    fn consoles_keep_their_order_and_options_whole() {
        let ports = machine();
        let asked = [("ttyS1".to_string(), Some("57600".to_string())), ("ttyS0".to_string(), None)];
        let consoles = consoles_of(&asked, &ports).unwrap();
        assert_eq!(
            consoles,
            [
                Console { port: "ttyS1".into(), options: "57600n8".into() },
                Console { port: "ttyS0".into(), options: "115200n8".into() },
            ]
        );
    }

    #[test]
    fn a_console_must_be_a_port_there_is_with_options_linux_reads_named_once() {
        let ports = machine();
        let refuse = |asked: &[(&str, Option<&str>)]| {
            let asked: Vec<(String, Option<String>)> = asked
                .iter()
                .map(|(name, options)| (name.to_string(), options.map(str::to_string)))
                .collect();
            consoles_of(&asked, &ports).unwrap_err().message
        };
        assert_eq!(
            refuse(&[("ttyS7", None)]),
            "no ttyS7: this computer's serial ports are ttyS0, ttyS1, ttyS4"
        );
        assert_eq!(refuse(&[("ttyS1", None), ("ttyS1", Some("9600"))]), "ttyS1 is named twice");
        assert!(refuse(&[("ttyS1", Some("fast"))]).starts_with("console=ttyS1,fast: "));
        assert!(refuse(&[("ttyS1", Some("115200n9"))]).contains("data bits"));
        assert_eq!(
            known("ttyS0", &[]).unwrap_err().message,
            "no ttyS0: this computer has no serial ports"
        );
    }

    #[test]
    fn the_menu_follows_the_consoles_that_grub_can_show() {
        let mut ports = machine();
        ports.push(port("ttyS5", 0xF0E8, "amt-sol", Some("0000:00:16.3")));
        let console = |port: &str, options: &str| Console {
            port: port.to_string(),
            options: options.to_string(),
        };
        let consoles = [
            console("ttyS1", "57600n8"),
            console("ttyS4", "9600e7"),
            console("ttyS5", "115200n8"),
            console("ttyS0", "115200n8"),
        ];
        let menu = menu_of(&consoles, &ports);
        let shown: Vec<(&str, u32)> = menu.iter().map(|m| (m.grub.as_str(), m.speed)).collect();
        // ttyS4 runs at even parity, and ttyS5 is not its function's first.
        assert_eq!(shown, [("com1", 57_600), ("com0", 115_200)]);
        assert_eq!(
            off_the_menu(&consoles[1], &ports, &menu),
            Some("GRUB shows it only at 8 data bits and no parity")
        );
        assert_eq!(
            off_the_menu(&consoles[2], &ports, &menu),
            Some("GRUB shows it only on a PCI card's first port")
        );
        assert_eq!(off_the_menu(&consoles[0], &ports, &menu), None);
    }

    #[test]
    fn a_console_grub_has_no_name_for_says_why() {
        // A port on a second PCI domain, and a kind of port newer than
        // this serial.
        let ports = vec![
            port("ttyS4", 0xE000, "pci", Some("0001:03:00.0")),
            port("ttyS5", 0xD000, "oxpcie", Some("0000:04:00.0")),
        ];
        let console = |port: &str, options: &str| Console {
            port: port.to_string(),
            options: options.to_string(),
        };
        let consoles = [console("ttyS4", "115200n8"), console("ttyS5", "115200n8")];
        let menu = menu_of(&consoles, &ports);
        assert!(menu.is_empty(), "{menu:?}");
        assert_eq!(
            off_the_menu(&consoles[0], &ports, &menu),
            Some("GRUB shows no port on a PCI domain other than 0000")
        );
        assert_eq!(off_the_menu(&consoles[1], &ports, &menu), Some("GRUB has no name for it"));
    }

    #[test]
    fn the_menu_has_four_ports_at_most() {
        let mut ports = vec![
            port("ttyS0", 0x3F8, "com1", None),
            port("ttyS1", 0x2F8, "com2", None),
            port("ttyS2", 0x3E8, "com3", None),
            port("ttyS3", 0x2E8, "com4", None),
        ];
        for (at, address) in ["0000:03:00.0", "0000:04:00.0"].iter().enumerate() {
            ports.push(port(&format!("ttyS{}", 4 + at), 0xE000, "pci", Some(address)));
        }
        let consoles: Vec<Console> = ports
            .iter()
            .map(|port| Console { port: port.name.clone(), options: "9600n8".to_string() })
            .collect();
        let menu = menu_of(&consoles, &ports);
        assert_eq!(menu.len(), MENU_PORTS);
        assert_eq!(menu[3].grub, "com3");
        assert_eq!(
            off_the_menu(&consoles[5], &ports, &menu),
            Some("GRUB shows it on four ports at most")
        );
    }

    #[test]
    fn the_menu_of_the_firmwares_console_is_its_port_else_com1() {
        let mut ports = machine();
        assert_eq!(auto_menu(&ports), [MenuPort { grub: "com0".into(), speed: 115_200 }]);
        ports[1].firmware = Some("57600n8".to_string());
        assert_eq!(auto_menu(&ports), [MenuPort { grub: "com1".into(), speed: 57_600 }]);
        // A firmware port GRUB cannot show leaves COM1's menu.
        ports[1].firmware = None;
        ports[2].firmware = Some("115200n8".to_string());
        ports[2].pci = Some("0001:00:16.3".to_string());
        assert_eq!(auto_menu(&ports), [MenuPort { grub: "com0".into(), speed: 115_200 }]);
        // No COM1, no firmware port: no menu.
        assert!(auto_menu(&ports[1..2]).is_empty());
    }

    #[test]
    fn changing_the_consoles_keeps_the_log_and_changing_the_log_keeps_the_consoles() {
        let ports = machine();
        let log = Change::Log(Log::Ports(vec!["ttyS0".to_string()]));
        let current = apply(None, &ports, &log).unwrap();
        assert_eq!(current.consoles, Consoles::Auto);
        assert_eq!(current.log, Log::Ports(vec!["ttyS0".to_string()]));
        assert_eq!(current.menu, auto_menu(&ports));
        let console = Change::Consoles(vec![("ttyS1".to_string(), Some("57600".to_string()))]);
        let both = apply(Some(current), &ports, &console).unwrap();
        assert_eq!(both.log, Log::Ports(vec!["ttyS0".to_string()]));
        assert_eq!(both.menu, [MenuPort { grub: "com1".into(), speed: 57_600 }]);
        let off = apply(Some(both.clone()), &ports, &Change::Off).unwrap();
        assert_eq!((off.consoles, off.log, off.menu), (Consoles::Off, both.log.clone(), vec![]));
        let back = apply(Some(both), &ports, &Change::Auto).unwrap();
        assert_eq!(back.consoles, Consoles::Auto);
        assert_eq!(back.menu, auto_menu(&ports));
        let nine = Change::Log(Log::Ports(vec!["ttyS9".to_string()]));
        assert!(apply(None, &ports, &nine).is_err());
    }

    #[test]
    fn the_list_has_each_port_with_the_console_it_has_now() {
        let mut ports = machine();
        ports[1].firmware = Some("57600n8".to_string());
        let line = "lockdep console=ttyS0 console=ttyS1,57600 quiet";
        let text = table(&ports, line);
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(
            rows[0],
            "PORT   WHAT                                           LOG  CONSOLE NOW"
        );
        assert_eq!(rows[1], "ttyS0  COM1 at I/O port 0x3F8                         yes  115200n8");
        assert_eq!(
            rows[2],
            "ttyS1  COM2 at I/O port 0x2F8                         no   primary, 57600n8"
        );
        assert_eq!(rows[3], "ttyS4  Intel AMT Serial-over-LAN at PCI 0000:00:16.3  yes  -");
    }

    #[test]
    fn the_console_now_is_primary_firmware_any_or_none() {
        let mut ports = machine();
        ports[1].firmware = Some("57600n8".to_string());
        let now = |line: &str| -> Vec<String> {
            let listed: Vec<_> =
                ports.iter().map(|port| (port.name.clone(), port.firmware.clone())).collect();
            let choice = serial_console::choose(line, &listed);
            ports.iter().map(|port| console_now(&choice, line, &port.name)).collect()
        };
        // The last console= is the primary console.
        assert_eq!(now("console=ttyS1,57600 console=ttyS0"), ["primary, 115200n8", "57600n8", "-"]);
        // A screen's console= last leaves no primary serial console.
        assert_eq!(now("console=ttyS0 console=tty0"), ["115200n8", "-", "-"]);
        // No console= and no spcr=off: the firmware's port.
        assert_eq!(now("quiet"), ["-", "firmware, 57600n8", "-"]);
        assert_eq!(now("quiet spcr=off"), ["-", "-", "-"]);
        assert_eq!(now("quiet serial-console=any"), ["any", "any", "any"]);
        assert_eq!(now("quiet -- console=ttyS0"), ["-", "firmware, 57600n8", "-"]);
    }

    #[test]
    fn the_next_start_says_consoles_log_and_menu() {
        let mut ports = machine();
        ports[1].firmware = Some("57600n8".to_string());
        let console = |port: &str, options: &str| Console {
            port: port.to_string(),
            options: options.to_string(),
        };
        let settings = Settings {
            consoles: Consoles::Ports(vec![
                console("ttyS1", "57600n8"),
                console("ttyS0", "115200n8"),
            ]),
            log: Log::All,
            menu: vec![
                MenuPort { grub: "com1".into(), speed: 57_600 },
                MenuPort { grub: "com0".into(), speed: 115_200 },
            ],
        };
        assert_eq!(
            settings_text(&settings, &ports),
            "At the next start (boot/grub/serial.env):\n  \
             Consoles:   ttyS1 at 57600n8, ttyS0 at 115200n8 (primary)\n  \
             Kernel log: all ports\n  \
             Boot menu:  com1 at 57600 baud, com0 at 115200 baud\n"
        );
        let auto = Settings { consoles: Consoles::Auto, log: Log::None, menu: Vec::new() };
        let text = settings_text(&auto, &ports);
        assert!(text.contains("Consoles:   the firmware's console port, ttyS1\n"), "{text}");
        assert!(text.contains("Kernel log: no port\n") && text.contains("Boot menu:  none\n"));
        let off = Settings {
            consoles: Consoles::Off,
            log: Log::Ports(vec!["ttyS0".into(), "ttyS4".into()]),
            menu: Vec::new(),
        };
        let text = settings_text(&off, &machine());
        assert!(text.contains("Consoles:   none\n") && text.contains("Kernel log: ttyS0, ttyS4\n"));
        let auto_none = settings_text(&auto, &machine());
        assert!(auto_none.contains("Consoles:   the firmware's console port\n"), "{auto_none}");
    }

    #[test]
    fn a_console_without_the_menu_says_why() {
        let mut ports = machine();
        ports.push(port("ttyS5", 0xF0E8, "amt-sol", Some("0000:00:16.3")));
        let settings = Settings {
            consoles: Consoles::Ports(vec![Console {
                port: "ttyS5".into(),
                options: "115200n8".into(),
            }]),
            log: Log::All,
            menu: Vec::new(),
        };
        assert!(settings_text(&settings, &ports).ends_with(
            "  ttyS5 has a console but not the boot menu: GRUB shows it only on a PCI card's \
             first port.\n"
        ));
    }

    #[test]
    fn what_is_known_of_the_next_start_without_a_block() {
        let ports = machine();
        assert_eq!(
            next_text(&Next::NoRoot, &ports),
            "At the next start: this system did not start from a Dukovo root on a disk.\n"
        );
        assert_eq!(
            next_text(&Next::NoBlock, &ports),
            "At the next start: as this one, since the boot menu has no serial settings \
             (boot/grub/serial.env).\n"
        );
        assert!(next_text(&Next::Edited, &ports).contains("the next change replaces them"));
        assert_eq!(
            next_text(&Next::NotRoot, &ports),
            "At the next start: only root reads the boot menu's serial settings, as `sudo serial`.\n"
        );
        assert!(
            next_text(&Next::Unreadable("it holds no FAT".into()), &ports)
                .ends_with("it holds no FAT\n")
        );
    }

    #[test]
    fn the_settings_written_with_esp_are_the_next_start_read_back() {
        let dir = std::env::temp_dir().join(format!("serial-esp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(read_next(Some(&dir)), Next::NoBlock));
        let settings = Settings {
            consoles: Consoles::Ports(vec![Console {
                port: "ttyS1".to_string(),
                options: "57600n8".to_string(),
            }]),
            log: Log::Ports(vec!["ttyS1".to_string()]),
            menu: vec![MenuPort { grub: "com1".into(), speed: 57_600 }],
        };
        assert!(write(&settings, Some(&dir)).unwrap().is_empty());
        assert!(matches!(read_next(Some(&dir)), Next::Block(read) if read == settings));
        // A block edited by hand into something else is no settings.
        std::fs::write(dir.join(SERIAL_ENV), "dukovo_console=console=ttyS1\n").unwrap();
        assert!(matches!(read_next(Some(&dir)), Next::Edited));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_ports_are_read_by_number_from_the_kernels_files() {
        let dir = std::env::temp_dir().join(format!("serial-ports-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, files) in [
            (
                "ttyS10",
                vec![
                    ("port", "0xE000\n"),
                    ("console", "N\n"),
                    ("kind", "pci\n"),
                    ("pci", "0000:04:00.0\n"),
                ],
            ),
            ("ttyS2", vec![("port", "0x3E8\n"), ("console", "Y\n"), ("kind", "com3\n")]),
            (
                "ttyS1",
                vec![
                    ("port", "0x2F8\n"),
                    ("console", "Y\n"),
                    ("kind", "com2\n"),
                    ("firmware_console", "57600n8\n"),
                ],
            ),
            ("ttyS9", vec![("port", "0xD000\n")]),
        ] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            for (file, text) in files {
                std::fs::write(dir.join(name).join(file), text).unwrap();
            }
        }
        std::fs::write(dir.join("ttySx"), "").unwrap();
        let found = ports(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        let names: Vec<&str> = found.iter().map(|port| port.name.as_str()).collect();
        // ttyS9 lacks the files the kernel always writes, so it is left out.
        assert_eq!(names, ["ttyS1", "ttyS2", "ttyS10"]);
        assert_eq!(found[0].firmware.as_deref(), Some("57600n8"));
        assert!(found[0].log && !found[2].log);
        assert_eq!(found[2].pci.as_deref(), Some("0000:04:00.0"));
        assert_eq!(found[2].base, 0xE000);
    }
}
