// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The serial console's settings a system starts with (ADR 0481): the
//! kernel flags that give serial ports a console and choose the ports that
//! carry the kernel log, and the ports GRUB shows its menu on, each at its
//! speed. They are a GRUB environment block ([`crate::envblock`]),
//! `boot/grub/serial.env` on each EFI system partition beside the menu,
//! which the menu reads with `load_env` (image/grub/installed.cfg.in) and
//! serial(8) and the installer write.
//!
//! A block holds `dukovo_console`, the flags as the menu puts them on each
//! kernel line, and `serial_0` to `serial_3` with `serial_0_speed` to
//! `serial_3_speed`, the menu's ports in GRUB's names and their speeds in
//! baud; a slot the menu does not use is empty. A system with no block
//! starts as it was installed before the block: the menu on COM1 at 115200
//! baud, and the console flags the menu's `@CONSOLE@` was filled with.
//!
//! The crate is without the standard library and reads no files. When a
//! start ignores the settings, `serial` shows what the next start will do
//! from the block, and the boot test install-serial checks that GRUB reads
//! one.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::envblock::{self, BLOCK};

/// Where the block is on an EFI system partition, beside the menu.
pub const PATH: &str = "boot/grub/serial.env";

/// The most ports GRUB shows its menu on: the block's slots.
pub const MENU_PORTS: usize = 4;

/// The variables the block holds, which the boot menu loads by name: the
/// kernel flags, then the port and speed of each menu slot.
pub const VARIABLES: [&str; 1 + 2 * MENU_PORTS] = [
    "dukovo_console",
    "serial_0",
    "serial_0_speed",
    "serial_1",
    "serial_1_speed",
    "serial_2",
    "serial_2_speed",
    "serial_3",
    "serial_3_speed",
];

/// The block's first comment line.
const COMMENT: &str = "# The serial console's settings, which serial(8) changes.";

/// The serial console's settings a system starts with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The ports that have a console.
    pub consoles: Consoles,
    /// The ports that carry the kernel log.
    pub log: Log,
    /// The ports GRUB shows its menu on, in order.
    pub menu: Vec<MenuPort>,
}

/// Which serial ports have a console.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Consoles {
    /// No `console=`: the firmware's console port, if it names one.
    Auto,
    /// `spcr=off` and no `console=`: none.
    Off,
    /// A `console=` for each, the primary console last. Never empty.
    Ports(Vec<Console>),
}

/// A serial port that has a console, and the line it runs at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Console {
    /// The port's name: `ttyS1`.
    pub port: String,
    /// Linux's console options, always whole: `57600n8`.
    pub options: String,
}

/// Which serial ports carry the kernel log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Log {
    /// Every port, the kernel's default.
    All,
    /// No port.
    None,
    /// These ports alone, by name. Never empty.
    Ports(Vec<String>),
}

/// A serial port GRUB shows its menu on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuPort {
    /// GRUB's name for it: `com1`, `pci,00:16.3`.
    pub grub: String,
    /// Its speed in baud.
    pub speed: u32,
}

impl Settings {
    /// The settings of a system with no block: the firmware's console, the
    /// log on every port, and `menu`.
    #[must_use]
    pub fn new(menu: Vec<MenuPort>) -> Settings {
        Settings { consoles: Consoles::Auto, log: Log::All, menu }
    }

    /// The kernel flags, each after a space, as `dukovo_console` holds
    /// them for the menu's kernel lines: none for [`Consoles::Auto`] and
    /// [`Log::All`]; ` spcr=off`; ` console=ttyS1,57600n8
    /// console=ttyS0,115200n8`; and then ` serial-log=none` or
    /// ` serial-log=ttyS0,ttyS4` when the log is not on every port.
    #[must_use]
    pub fn kernel_flags(&self) -> String {
        let mut flags = String::new();
        match &self.consoles {
            Consoles::Auto => {}
            Consoles::Off => flags.push_str(" spcr=off"),
            Consoles::Ports(consoles) => {
                for Console { port, options } in consoles {
                    flags.push_str(&format!(" console={port},{options}"));
                }
            }
        }
        match &self.log {
            Log::All => {}
            Log::None => flags.push_str(" serial-log=none"),
            Log::Ports(ports) => flags.push_str(&format!(" serial-log={}", ports.join(","))),
        }
        flags
    }

    /// The settings as a block. `None` if they do not fit: more than
    /// [`MENU_PORTS`] menu ports, or flags too long for the block.
    #[must_use]
    pub fn encode(&self) -> Option<[u8; BLOCK]> {
        if self.menu.len() > MENU_PORTS {
            return None;
        }
        let mut values = Vec::from([self.kernel_flags()]);
        for slot in 0..MENU_PORTS {
            let port = self.menu.get(slot);
            values.push(port.map_or_default(|port| port.grub.clone()));
            values.push(port.map_or_default(|port| port.speed.to_string()));
        }
        let variables: Vec<(&str, &str)> =
            VARIABLES.iter().copied().zip(values.iter().map(String::as_str)).collect();
        envblock::write(&[COMMENT], &variables)
    }

    /// The settings a block holds. `None` if `bytes` is not a GRUB
    /// environment block, or one that was edited by hand: `dukovo_console`
    /// missing or with a word other than `console=ttyS<n>,<options>`,
    /// `spcr=off` or `serial-log=`, or a menu port whose speed is not a
    /// number.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Settings> {
        let variables = envblock::parse(bytes)?;
        let (consoles, log) = parse_flags(envblock::get(&variables, VARIABLES[0])?)?;
        let mut menu = Vec::new();
        for slot in 0..MENU_PORTS {
            let (port, speed) = (VARIABLES[1 + 2 * slot], VARIABLES[2 + 2 * slot]);
            let grub = envblock::get(&variables, port).unwrap_or("");
            if grub.is_empty() {
                continue;
            }
            let speed = envblock::get(&variables, speed)?;
            menu.push(MenuPort { grub: String::from(grub), speed: speed.parse().ok()? });
        }
        Some(Settings { consoles, log, menu })
    }
}

/// The consoles and the log that kernel flags `flags` ask for.
fn parse_flags(flags: &str) -> Option<(Consoles, Log)> {
    let mut ports = Vec::new();
    let mut spcr_off = false;
    let mut log = Log::All;
    for word in flags.split_whitespace() {
        if word == "spcr=off" {
            spcr_off = true;
        } else if let Some(value) = word.strip_prefix("console=") {
            let (port, options) = value.split_once(',')?;
            if !is_port(port) || options.is_empty() {
                return None;
            }
            ports.push(Console { port: String::from(port), options: String::from(options) });
        } else {
            // Any other word is not one `serial` writes.
            let value = word.strip_prefix("serial-log=")?;
            log = match value {
                "all" => Log::All,
                "none" => Log::None,
                list => Log::Ports(
                    list.split(',')
                        .map(|port| is_port(port).then(|| String::from(port)))
                        .collect::<Option<_>>()?,
                ),
            };
        }
    }
    let consoles = if !ports.is_empty() {
        Consoles::Ports(ports)
    } else if spcr_off {
        Consoles::Off
    } else {
        Consoles::Auto
    };
    Some((consoles, log))
}

/// Whether `name` is a serial port's, as the kernel names them: `ttyS`
/// and a number with no leading zero.
fn is_port(name: &str) -> bool {
    name.strip_prefix("ttyS").is_some_and(|digits| {
        !digits.is_empty()
            && digits.bytes().all(|byte| byte.is_ascii_digit())
            && (digits == "0" || !digits.starts_with('0'))
    })
}

/// GRUB's name for the port the kernel calls `kind` in
/// `/kernel/class/tty/ttyS<n>/kind`, at PCI function `pci` from its `pci`
/// file: `com1` to `com4` are GRUB's `com0` to `com3`; a port on PCI
/// (`amt-sol`, `ax99100`, `qemu`, `pci`) at `0000:BB:DD.F` is `pci,BB:DD.F`
/// when it is its function's first port, `first`, as GRUB runs only that
/// one. Anything else has no name: a port of another PCI domain, which
/// GRUB's `pci,` form cannot say, or one the BIOS does not name.
#[must_use]
pub fn grub_name(kind: &str, pci: Option<&str>, first: bool) -> Option<String> {
    if let Some(number) = kind.strip_prefix("com") {
        let number: u8 = number.parse().ok().filter(|number| (1..=4).contains(number))?;
        return Some(format!("com{}", number - 1));
    }
    if !matches!(kind, "amt-sol" | "ax99100" | "qemu" | "pci") || !first {
        return None;
    }
    let (domain, rest) = pci?.trim().split_once(':')?;
    let (bus, rest) = rest.split_once(':')?;
    let (device, function) = rest.split_once('.')?;
    let hex = |text: &str, digits: usize| {
        text.len() == digits && text.bytes().all(|byte| byte.is_ascii_hexdigit())
    };
    if domain != "0000" || !hex(bus, 2) || !hex(device, 2) || !hex(function, 1) {
        return None;
    }
    Some(format!("pci,{bus}:{device}.{function}").to_ascii_lowercase())
}

/// Whether GRUB can show its menu at the console options `options`: it
/// runs a port at 8 data bits and no parity only, as in `57600`,
/// `57600n8` and `57600n8r`.
#[must_use]
pub fn menu_line(options: &str) -> bool {
    let digits = options.find(|c: char| !c.is_ascii_digit()).unwrap_or(options.len());
    matches!(&options[digits..], "" | "n" | "n8" | "n8r")
}

/// The speed in baud that console options `options` start with.
#[must_use]
pub fn speed(options: &str) -> Option<u32> {
    let digits = options.find(|c: char| !c.is_ascii_digit()).unwrap_or(options.len());
    options[..digits].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console(port: &str, options: &str) -> Console {
        Console { port: port.into(), options: options.into() }
    }

    fn menu_port(grub: &str, speed: u32) -> MenuPort {
        MenuPort { grub: grub.into(), speed }
    }

    fn settings(consoles: Consoles, log: Log) -> Settings {
        Settings { consoles, log, menu: Vec::from([menu_port("com0", 115_200)]) }
    }

    #[test]
    fn every_kind_of_console_and_log_reads_back_as_written() {
        let consoles = [
            Consoles::Auto,
            Consoles::Off,
            Consoles::Ports(Vec::from([console("ttyS1", "57600n8")])),
            Consoles::Ports(Vec::from([
                console("ttyS1", "57600n8"),
                console("ttyS4", "9600e7r"),
                console("ttyS0", "115200n8"),
            ])),
        ];
        let logs = [
            Log::All,
            Log::None,
            Log::Ports(Vec::from([String::from("ttyS0")])),
            Log::Ports(Vec::from([String::from("ttyS0"), String::from("ttyS4")])),
        ];
        for consoles in &consoles {
            for log in &logs {
                let before = settings(consoles.clone(), log.clone());
                let block = before.encode().unwrap();
                assert_eq!(Settings::decode(&block), Some(before));
            }
        }
    }

    #[test]
    fn the_kernel_flags_are_each_after_a_space() {
        let flags = |consoles, log| settings(consoles, log).kernel_flags();
        assert_eq!(flags(Consoles::Auto, Log::All), "");
        assert_eq!(flags(Consoles::Off, Log::All), " spcr=off");
        let two =
            Consoles::Ports(Vec::from([console("ttyS1", "57600n8"), console("ttyS0", "115200n8")]));
        assert_eq!(flags(two.clone(), Log::All), " console=ttyS1,57600n8 console=ttyS0,115200n8");
        assert_eq!(flags(Consoles::Auto, Log::None), " serial-log=none");
        let some = Log::Ports(Vec::from([String::from("ttyS0"), String::from("ttyS4")]));
        assert_eq!(flags(Consoles::Off, some.clone()), " spcr=off serial-log=ttyS0,ttyS4");
        assert_eq!(
            flags(two, some),
            " console=ttyS1,57600n8 console=ttyS0,115200n8 serial-log=ttyS0,ttyS4"
        );
    }

    #[test]
    fn a_system_with_no_block_starts_as_before() {
        let new = Settings::new(Vec::from([menu_port("com0", 115_200)]));
        assert_eq!(new.consoles, Consoles::Auto);
        assert_eq!(new.log, Log::All);
        assert_eq!(new.kernel_flags(), "");
    }

    #[test]
    fn the_block_has_four_menu_slots_and_no_more() {
        let four: Vec<MenuPort> = ["com0", "com1", "com2", "pci,00:16.3"]
            .iter()
            .zip([115_200, 57_600, 9_600, 38_400])
            .map(|(grub, speed)| menu_port(grub, speed))
            .collect();
        let full = Settings { menu: four, ..Settings::new(Vec::new()) };
        let block = full.encode().unwrap();
        let variables = envblock::parse(&block).unwrap();
        assert_eq!(envblock::get(&variables, "serial_3"), Some("pci,00:16.3"));
        assert_eq!(envblock::get(&variables, "serial_3_speed"), Some("38400"));
        assert_eq!(Settings::decode(&block), Some(full.clone()));
        let five = Settings {
            menu: [full.menu.clone(), Vec::from([menu_port("com3", 9_600)])].concat(),
            ..full
        };
        assert_eq!(five.encode(), None);
    }

    #[test]
    fn the_block_holds_the_variables_the_menu_loads_and_no_others() {
        let block = Settings::new(Vec::from([menu_port("com0", 115_200)])).encode().unwrap();
        let names: Vec<String> =
            envblock::parse(&block).unwrap().into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, VARIABLES);
    }

    #[test]
    fn a_slot_the_menu_does_not_use_is_empty() {
        let block = Settings::new(Vec::from([menu_port("com1", 57_600)])).encode().unwrap();
        assert!(block.starts_with(b"# GRUB Environment Block\n# The serial console's settings"));
        let variables = envblock::parse(&block).unwrap();
        assert_eq!(envblock::get(&variables, "dukovo_console"), Some(""));
        assert_eq!(envblock::get(&variables, "serial_0"), Some("com1"));
        assert_eq!(envblock::get(&variables, "serial_0_speed"), Some("57600"));
        for slot in 1..MENU_PORTS {
            assert_eq!(envblock::get(&variables, &format!("serial_{slot}")), Some(""));
            assert_eq!(envblock::get(&variables, &format!("serial_{slot}_speed")), Some(""));
        }
    }

    #[test]
    fn a_block_edited_by_hand_is_not_read() {
        let block = |flags: &str, speed: &str| {
            envblock::write(
                &[],
                &[("dukovo_console", flags), ("serial_0", "com0"), ("serial_0_speed", speed)],
            )
            .unwrap()
        };
        assert!(Settings::decode(&block(" console=ttyS1,57600n8", "115200")).is_some());
        // Words serial(8) never writes.
        assert_eq!(Settings::decode(&block(" console=tty0", "115200")), None);
        assert_eq!(Settings::decode(&block(" console=ttyS1", "115200")), None);
        assert_eq!(Settings::decode(&block(" console=ttyS01,9600", "115200")), None);
        assert_eq!(Settings::decode(&block(" quiet", "115200")), None);
        assert_eq!(Settings::decode(&block(" serial-log=ttyS0,tty1", "115200")), None);
        assert_eq!(Settings::decode(&block(" serial-log=", "115200")), None);
        // A speed that is no number.
        assert_eq!(Settings::decode(&block("", "fast")), None);
        assert_eq!(Settings::decode(&block("", "")), None);
        // Not a block at all, or one without the flags.
        assert_eq!(Settings::decode(b"not a block"), None);
        assert_eq!(Settings::decode(&envblock::write(&[], &[("serial_0", "com0")]).unwrap()), None);
    }

    #[test]
    fn spcr_off_beside_a_console_leaves_the_console() {
        let block = envblock::write(&[], &[("dukovo_console", " spcr=off console=ttyS1,9600")]);
        let read = Settings::decode(&block.unwrap()).unwrap();
        assert_eq!(read.consoles, Consoles::Ports(Vec::from([console("ttyS1", "9600")])));
        let all = envblock::write(&[], &[("dukovo_console", " serial-log=all")]).unwrap();
        assert_eq!(Settings::decode(&all).unwrap().log, Log::All);
    }

    #[test]
    fn grub_names_the_com_ports_from_zero_and_the_pci_function_by_address() {
        assert_eq!(grub_name("com1", None, true).as_deref(), Some("com0"));
        assert_eq!(grub_name("com4", None, true).as_deref(), Some("com3"));
        assert_eq!(grub_name("com5", None, true), None);
        assert_eq!(grub_name("com0", None, true), None);
        let amt = grub_name("amt-sol", Some("0000:00:16.3"), true);
        assert_eq!(amt.as_deref(), Some("pci,00:16.3"));
        assert_eq!(grub_name("qemu", Some("0000:00:04.0"), true).as_deref(), Some("pci,00:04.0"));
        assert_eq!(
            grub_name("ax99100", Some("0000:0A:00.1"), true).as_deref(),
            Some("pci,0a:00.1")
        );
        // GRUB runs the first port of a function only.
        assert_eq!(grub_name("amt-sol", Some("0000:00:16.3"), false), None);
        // Its `pci,` form has no PCI domain.
        assert_eq!(grub_name("amt-sol", Some("0001:00:16.3"), true), None);
        assert_eq!(grub_name("pci", None, true), None);
        assert_eq!(grub_name("pci", Some("00:16.3"), true), None);
        assert_eq!(grub_name("pci", Some("0000:0:16.3"), true), None);
        assert_eq!(grub_name("pci", Some("0000:00:16.13"), true), None);
        assert_eq!(grub_name("uart", Some("0000:00:16.3"), true), None);
    }

    #[test]
    fn the_menu_runs_at_eight_data_bits_and_no_parity() {
        for options in ["57600", "57600n8", "57600n8r", "115200n"] {
            assert!(menu_line(options), "{options}");
        }
        for options in ["9600e7", "115200o8", "9600n7", "9600e8"] {
            assert!(!menu_line(options), "{options}");
        }
    }

    #[test]
    fn the_speed_is_the_options_leading_digits() {
        assert_eq!(speed("57600n8"), Some(57_600));
        assert_eq!(speed("9600"), Some(9_600));
        assert_eq!(speed("n8"), None);
        assert_eq!(speed(""), None);
    }
}
