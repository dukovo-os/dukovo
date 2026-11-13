// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Braille displays (ADR 0326): brailled(8), which starts BRLTTY when a
//! display is plugged in, and braille(1), which turns it on and off and
//! names the display's driver, its port and the text table in
//! braille.toml(5).
//!
//! brailled looks every [`POLL`] for a display: one plugged in by USB
//! whose IDs [`DISPLAYS`] knows, as the kernel shows it in
//! _/kernel/bus/usb/devices_ with the terminal its serial port is
//! (radix-sysfs(5)), or the port and driver braille.toml names, for a
//! display on a serial port or one whose IDs are a chip's that others use
//! too. It runs brltty(1) for it, with that driver and the screen driver
//! for Dukovo, which reads the consoles from consoled(8). When the
//! display goes, or another is plugged in in its place, or the settings
//! change, it stops BRLTTY and starts it again as they now say. With
//! `on = false` it ends at once, which leaves its service stopped.

use std::io;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use audio::settings::{entry, string};

use crate::args::Args;
use crate::util::{self, Fail, fail, usage};

/// Where the settings are: state, as reader.toml(5) is.
pub const PATH: &str = "/system/state/braille/braille.toml";

/// What a new settings file starts with.
const HEADER: &str = "# Braille displays (braille.toml(5)): whether brailled(8) starts BRLTTY\n\
                      # for one, its braille driver and port (auto: found by its USB IDs),\n\
                      # and the text table. braille(1) changes it.\n";

/// BRLTTY.
const BRLTTY: &str = "/system/commands/brltty";

/// BRLTTY's own configuration file, which a person may write; BRLTTY
/// counts it missing as an error, so it is given an empty one instead.
const CONFIGURATION: &str = "/system/settings/braille/brltty.conf";

/// Where the kernel shows the USB devices.
const USB_DEVICES: &str = "/kernel/bus/usb/devices";

/// How often brailled looks for a display, and reads the settings again.
const POLL: Duration = Duration::from_secs(2);

/// How long brailled waits before starting BRLTTY again for the same
/// display, after it ended.
const RETRY: Duration = Duration::from_secs(10);

/// The displays brailled finds by their USB IDs: vendor, product,
/// BRLTTY's driver and the display's name, as BRLTTY's drivers list them.
/// Each is a serial port to the kernel: an FTDI chip under an ID of its
/// maker's, or a CDC-ACM device. A display on a chip that others use as
/// well, such as FTDI's FT232R as it leaves FTDI (0403:6001), is not
/// guessed at: braille.toml names its driver and port.
pub const DISPLAYS: [(u16, u16, &str, &str); 34] = [
    (0x0403, 0xde58, "hd", "Hedo MobilLine"),
    (0x0403, 0xde59, "hd", "Hedo ProfiLine"),
    (0x0403, 0xf208, "pm", "Papenmeier"),
    (0x0403, 0xfe70, "bm", "Baum Vario 40"),
    (0x0403, 0xfe71, "bm", "Baum PocketVario"),
    (0x0403, 0xfe72, "bm", "Baum SuperVario 40"),
    (0x0403, 0xfe73, "bm", "Baum SuperVario 32"),
    (0x0403, 0xfe74, "bm", "Baum SuperVario 64"),
    (0x0403, 0xfe75, "bm", "Baum SuperVario 80"),
    (0x0403, 0xfe76, "bm", "Baum VarioPro 80"),
    (0x0403, 0xfe77, "bm", "Baum VarioPro 64"),
    (0x0904, 0x2000, "bm", "Baum VarioPro 40"),
    (0x0904, 0x2001, "bm", "Baum EcoVario 24"),
    (0x0904, 0x2002, "bm", "Baum EcoVario 40"),
    (0x0904, 0x2007, "bm", "Baum VarioConnect 40"),
    (0x0904, 0x2008, "bm", "Baum VarioConnect 32"),
    (0x0904, 0x2009, "bm", "Baum VarioConnect 24"),
    (0x0904, 0x2010, "bm", "Baum VarioConnect 64"),
    (0x0904, 0x2011, "bm", "Baum VarioConnect 80"),
    (0x0904, 0x2014, "bm", "Baum EcoVario 32"),
    (0x0904, 0x2015, "bm", "Baum EcoVario 64"),
    (0x0904, 0x2016, "bm", "Baum EcoVario 80"),
    (0x0904, 0x3000, "bm", "Baum Refreshabraille 18"),
    (0x16c0, 0x05e1, "cn", "Canute 360"),
    (0x1c71, 0xc005, "hw", "HumanWare Brailliant BI 32, 40 or B 80"),
    (0x1c71, 0xc021, "hw", "HumanWare Brailliant BI 14"),
    (0x1c71, 0xc104, "hw", "APH Chameleon 20"),
    (0x1c71, 0xc114, "hw", "APH Mantis Q40"),
    (0x1c71, 0xc124, "hw", "HumanWare BrailleOne"),
    (0x1c71, 0xce04, "hw", "NLS eReader"),
    (0x28ac, 0x0012, "eu", "Eurobraille b.note"),
    (0x28ac, 0x0013, "eu", "Eurobraille b.note 2"),
    (0x28ac, 0x0020, "eu", "Eurobraille b.book"),
    (0x28ac, 0x0021, "eu", "Eurobraille b.book, external"),
];

/// BRLTTY's braille drivers, by the codes braille.toml names them with.
pub const DRIVERS: [&str; 37] = [
    "al", "at", "bc", "bd", "bl", "bm", "bn", "cb", "ce", "cn", "dp", "ec", "eu", "fa", "fs", "hd",
    "hm", "ht", "hw", "ic", "ir", "lt", "mb", "md", "mm", "mn", "mt", "np", "pg", "pm", "sk", "tn",
    "ts", "tt", "vd", "vo", "vs",
];

/// The settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Whether brailled starts BRLTTY for a display.
    pub on: bool,
    /// BRLTTY's braille driver, or `auto`: the one the display's IDs say.
    pub driver: String,
    /// The display's port, a terminal in _/devices_, or `auto`: the one
    /// a display brailled knows by its IDs is.
    pub device: String,
    /// BRLTTY's text table: how each character is shown in dots.
    pub table: String,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            on: true,
            driver: "auto".to_string(),
            device: "auto".to_string(),
            table: "en-nabcc".to_string(),
        }
    }
}

/// Whether `driver` is one of BRLTTY's, or `auto`.
fn is_driver(driver: &str) -> bool {
    driver == "auto" || DRIVERS.contains(&driver)
}

/// Whether `device` is a terminal in _/devices_, or `auto`.
fn is_device(device: &str) -> bool {
    device == "auto"
        || device.strip_prefix("/devices/").is_some_and(|name| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '/')
        }) && !device.contains("//")
}

/// Whether `table` is the name of one of BRLTTY's text tables: letters,
/// digits, `-` and `_`, as _/system/resources/brltty/Text_ names them.
fn is_table(table: &str) -> bool {
    !table.is_empty()
        && table.len() <= 32
        && !table.starts_with('-')
        && table.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

impl Settings {
    /// Reads the settings from `text`: values missing or wrong are the
    /// defaults.
    pub fn parse(text: &str) -> Settings {
        let mut settings = Settings::default();
        for (key, value) in text.lines().filter_map(entry) {
            let text = string(value);
            match key {
                "on" => settings.on = value != "false",
                "driver" => {
                    if let Some(driver) = text.filter(|d| is_driver(d)) {
                        settings.driver = driver;
                    }
                }
                "device" => {
                    if let Some(device) = text.filter(|d| is_device(d)) {
                        settings.device = device;
                    }
                }
                "table" => {
                    if let Some(table) = text.filter(|t| is_table(t)) {
                        settings.table = table;
                    }
                }
                _ => {}
            }
        }
        settings
    }

    /// `text` with these settings in it: each key's line changed where it
    /// is, those it lacks added at the end, and the rest kept.
    pub fn update(&self, text: &str) -> String {
        let values = [
            ("on", self.on.to_string()),
            ("driver", format!("\"{}\"", self.driver)),
            ("device", format!("\"{}\"", self.device)),
            ("table", format!("\"{}\"", self.table)),
        ];
        let mut seen = [false; 4];
        let mut out = String::new();
        for line in text.lines() {
            let replaced = entry(line).and_then(|(key, _)| {
                let i = values.iter().position(|(name, _)| *name == key)?;
                seen[i] = true;
                Some(format!("{key} = {}", values[i].1))
            });
            out.push_str(replaced.as_deref().unwrap_or(line));
            out.push('\n');
        }
        for ((key, value), seen) in values.iter().zip(seen) {
            if !seen {
                out.push_str(&format!("{key} = {value}\n"));
            }
        }
        out
    }
}

/// Reads the settings: the defaults if the file is missing.
fn load() -> Settings {
    std::fs::read_to_string(PATH)
        .map_or_else(|_| Settings::default(), |text| Settings::parse(&text))
}

/// Writes `settings` over the file, keeping its notes: a new file put in
/// the old one's place, so that brailled never reads half of one.
fn save(settings: &Settings) -> io::Result<()> {
    let old = std::fs::read_to_string(PATH).unwrap_or_else(|_| HEADER.to_string());
    let dir = Path::new(PATH).parent().unwrap_or(Path::new("/"));
    std::fs::create_dir_all(dir)?;
    let new = format!("{PATH}.new");
    std::fs::write(&new, settings.update(&old))?;
    std::fs::rename(&new, PATH)
}

/// A display plugged in by USB, as brailled found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// BRLTTY's driver for it, and its name.
    pub driver: &'static str,
    pub name: &'static str,
    /// Its terminal: _/devices/ttyUSB0_.
    pub device: String,
}

/// The first display in `devices` (the kernel's _/kernel/bus/usb/devices_)
/// that [`DISPLAYS`] knows, and whose serial port has its terminal.
pub fn find(devices: &Path) -> Option<Found> {
    let mut names: Vec<String> = std::fs::read_dir(devices)
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    names.sort();
    let hex = |dir: &Path, file: &str| {
        let text = std::fs::read_to_string(dir.join(file)).ok()?;
        u16::from_str_radix(text.trim(), 16).ok()
    };
    for name in names.iter().filter(|name| !name.contains(':') && !name.starts_with("usb")) {
        let dir = devices.join(name);
        let (Some(vendor), Some(product)) = (hex(&dir, "idVendor"), hex(&dir, "idProduct")) else {
            continue;
        };
        let Some(&(_, _, driver, display)) =
            DISPLAYS.iter().find(|&&(v, p, _, _)| v == vendor && p == product)
        else {
            continue;
        };
        // Its interfaces, `1-2:1.0`, and the terminal one of them is:
        // `ttyUSB0` in it, or `tty/ttyACM0`.
        let prefix = format!("{name}:");
        for interface in names.iter().filter(|n| n.starts_with(&prefix)) {
            let at = devices.join(interface);
            if let Some(tty) = terminal(&at).or_else(|| terminal(&at.join("tty"))) {
                return Some(Found { driver, name: display, device: format!("/devices/{tty}") });
            }
        }
    }
    None
}

/// The name of the terminal in `dir`, `ttyUSB0` or `ttyACM0`, if there is
/// one.
fn terminal(dir: &Path) -> Option<String> {
    let mut found: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("ttyUSB") || name.starts_with("ttyACM"))
        .collect();
    found.sort();
    found.into_iter().next()
}

/// What BRLTTY is run with: its braille driver, the display's port and
/// the text table, and the display's name for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Start {
    pub driver: String,
    pub device: String,
    pub table: String,
    pub name: String,
}

impl Start {
    /// BRLTTY's arguments: in the foreground, logging to its standard
    /// error, which Spark keeps, the display's driver and port, the screen
    /// driver for Dukovo and the text table; and, while there is no
    /// [`CONFIGURATION`] file (`configured`), an empty one in its place.
    pub fn arguments(&self, configured: bool) -> Vec<String> {
        let device = format!("serial:{}", self.device);
        let mut arguments: Vec<String> =
            ["-n", "-e", "-b", &self.driver, "-d", &device, "-x", "dk", "-t", &self.table]
                .map(str::to_string)
                .to_vec();
        if !configured {
            arguments.extend(["-f".to_string(), "/devices/null".to_string()]);
        }
        arguments
    }
}

/// What BRLTTY should run with, by `settings` and the display `found`, if
/// anything: nothing while braille is off, or no display is there.
/// A port braille.toml names wants its driver named too.
pub fn wanted(settings: &Settings, found: Option<&Found>) -> Result<Option<Start>, String> {
    if !settings.on {
        return Ok(None);
    }
    let table = settings.table.clone();
    if settings.device != "auto" {
        if settings.driver == "auto" {
            return Err(format!(
                "braille.toml names the port {} but not its driver: `braille driver CODE` names it",
                settings.device
            ));
        }
        let name = format!("the display on {}", settings.device);
        return Ok(Some(Start {
            driver: settings.driver.clone(),
            device: settings.device.clone(),
            table,
            name,
        }));
    }
    Ok(found.map(|found| {
        let driver = if settings.driver == "auto" {
            found.driver.to_string()
        } else {
            settings.driver.clone()
        };
        Start { driver, device: found.device.clone(), table, name: found.name.to_string() }
    }))
}

/// Set by SIGTERM: brailled stops BRLTTY and ends.
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// BRLTTY as brailled runs it.
struct Running {
    child: Child,
    start: Start,
}

/// Stops `running`'s BRLTTY and waits for it to end.
fn stop(mut running: Running, why: &str) {
    println!("{why}: stopping BRLTTY");
    // SAFETY: kill sends a signal to the child, which has not been reaped.
    unsafe { libc::kill(running.child.id() as libc::pid_t, libc::SIGTERM) };
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match running.child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) if Instant::now() >= until => {
                let _ = running.child.kill();
                let _ = running.child.wait();
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// brailled(8).
pub fn daemon(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    if !load().on {
        println!("braille displays are off (braille.toml(5)); `braille on` turns them on");
        return Ok(0);
    }
    // SAFETY: the handler only sets an atomic flag.
    unsafe {
        libc::signal(libc::SIGTERM, on_stop as *const () as libc::sighandler_t);
    }
    println!("looking for a braille display");
    let mut running: Option<Running> = None;
    // The last start that ended, and when it may be tried again.
    let mut ended: Option<(Start, Instant)> = None;
    let mut complained = String::new();
    loop {
        let settings = load();
        let found = find(Path::new(USB_DEVICES));
        let wanted = match wanted(&settings, found.as_ref()) {
            Ok(wanted) => wanted,
            Err(problem) => {
                if problem != complained {
                    println!("{problem}");
                    complained = problem;
                }
                None
            }
        };
        if let Some(mut now) = running.take() {
            // What changed is said first, whether or not BRLTTY saw it and
            // ended already.
            if wanted.as_ref() != Some(&now.start) {
                let why = match &wanted {
                    None if !settings.on => "braille is turned off".to_string(),
                    None => format!("{} was unplugged", now.start.name),
                    Some(start) => format!("{} is now used", start.name),
                };
                stop(now, &why);
            } else if let Ok(Some(status)) = now.child.try_wait() {
                println!("BRLTTY ended ({status})");
                ended = Some((now.start, Instant::now() + RETRY));
            } else {
                running = Some(now);
            }
        }
        if running.is_none()
            && let Some(start) = wanted
        {
            let waiting =
                ended.as_ref().is_some_and(|(last, at)| *last == start && Instant::now() < *at);
            if !waiting {
                println!(
                    "found {} on {}: starting BRLTTY with its {} driver",
                    start.name, start.device, start.driver
                );
                let configured = Path::new(CONFIGURATION).exists();
                match Command::new(BRLTTY).args(start.arguments(configured)).spawn() {
                    Ok(child) => running = Some(Running { child, start }),
                    Err(err) => {
                        println!("cannot start {BRLTTY}: {err}");
                        ended = Some((start, Instant::now() + RETRY));
                    }
                }
            }
        }
        let until = Instant::now() + POLL;
        while Instant::now() < until && !STOP.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
        if STOP.load(Ordering::Relaxed) {
            if let Some(now) = running.take() {
                stop(now, "brailled is stopping");
            }
            return Ok(0);
        }
    }
}

/// Starts or stops brailled's service, and returns once it has started
/// or stopped; service(8) says only what went wrong.
fn service(command: &str) -> Result<(), Fail> {
    let status = Command::new("/system/commands/service")
        .args(["brailled", command, "--quiet"])
        .status()
        .map_err(|e| util::failed(&"service", e))?;
    if status.success() {
        Ok(())
    } else {
        Err(fail(format!("`service brailled {command}` failed")))
    }
}

const USAGE: &str = "\
Usage: braille [status]
       braille on | off
       braille driver [CODE | auto]
       braille device [/devices/NAME | auto]
       braille table [NAME]
       braille displays

Turns braille displays on and off, and names a display's BRLTTY driver,
its port and the text table in /system/state/braille/braille.toml
(braille.toml(5)). brailled(8) starts BRLTTY for a display it finds.";

/// braille(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let command = args.subcommand().map(|c| c.to_string_lossy().into_owned());
    let rest: Vec<String> = args.rest().iter().map(|a| a.to_string_lossy().into_owned()).collect();
    if rest.iter().any(|a| a == "--help" || a == "-h") || command.as_deref() == Some("help") {
        println!("{USAGE}");
        return Ok(0);
    }
    if rest.len() > 1 {
        return Err(usage(format!("unexpected argument '{}'", rest[1])));
    }
    let first = rest.first().map(String::as_str);
    let mut settings = load();
    let saved = |settings: &Settings| save(settings).map_err(|e| util::failed(&PATH, e));
    match command.as_deref() {
        None | Some("status") => {
            println!("Braille: {}", if settings.on { "on" } else { "off" });
            println!("Driver:  {}", settings.driver);
            println!("Port:    {}", settings.device);
            println!("Table:   {}", settings.table);
            match wanted(&settings, find(Path::new(USB_DEVICES)).as_ref()) {
                Ok(Some(start)) => println!(
                    "Display: {} on {}, with BRLTTY's {} driver",
                    start.name, start.device, start.driver
                ),
                Ok(None) if settings.on => println!("Display: none plugged in"),
                Ok(None) => {}
                Err(problem) => println!("Display: {problem}"),
            }
        }
        Some("on") => {
            settings.on = true;
            saved(&settings)?;
            service("start")?;
            println!("on");
        }
        Some("off") => {
            settings.on = false;
            saved(&settings)?;
            service("stop")?;
            println!("off");
        }
        Some("driver") => match first {
            None => println!("{}", settings.driver),
            Some(driver) if is_driver(driver) => {
                settings.driver = driver.to_string();
                saved(&settings)?;
                println!("{driver}");
            }
            Some(driver) => {
                return Err(usage(format!(
                    "not one of BRLTTY's braille drivers: {driver} (`braille displays` lists them)"
                )));
            }
        },
        Some("device") => match first {
            None => println!("{}", settings.device),
            Some(device) if is_device(device) => {
                settings.device = device.to_string();
                saved(&settings)?;
                println!("{device}");
            }
            Some(device) => {
                return Err(usage(format!("not a terminal in /devices: {device}")));
            }
        },
        Some("table") => match first {
            None => println!("{}", settings.table),
            Some(table) if is_table(table) => {
                let file = format!("/system/resources/brltty/Text/{table}.ttb");
                if !Path::new(&file).exists() {
                    return Err(fail(format!("no text table {table}: {file} is not there")));
                }
                settings.table = table.to_string();
                saved(&settings)?;
                println!("{table}");
            }
            Some(table) => return Err(usage(format!("not a table's name: {table}"))),
        },
        Some("displays") => {
            if first.is_some() {
                return Err(usage(format!("unexpected argument '{}'", rest[0])));
            }
            println!("Found by their USB IDs:");
            for (vendor, product, driver, name) in DISPLAYS {
                println!("  {vendor:04x}:{product:04x}  {driver}  {name}");
            }
            println!("BRLTTY's drivers, for `braille driver`: {}", DRIVERS.join(" "));
        }
        Some(other) => return Err(usage(format!("unknown command '{other}'"))),
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_read_kept_and_wrong_values_are_the_defaults() {
        let text = "# notes\non = false\ndriver = \"bm\"\ndevice = \"/devices/ttyS0\"\n\
                    table = \"de\" # German\n";
        let settings = Settings::parse(text);
        assert_eq!(
            settings,
            Settings {
                on: false,
                driver: "bm".into(),
                device: "/devices/ttyS0".into(),
                table: "de".into()
            }
        );
        let wrong = "driver = \"zz\"\ndevice = \"/etc/passwd\"\ntable = \"-x\"\n";
        assert_eq!(Settings::parse(wrong), Settings::default());
        for device in ["/devices/../x", "/devices/", "/devices//ttyS0", "ttyS0"] {
            assert!(!is_device(device), "{device}");
        }
        // Writing keeps the notes and changes each key where it is.
        let updated = Settings { on: true, ..settings }.update(text);
        assert!(updated.starts_with("# notes\non = true\ndriver = \"bm\"\n"));
        assert_eq!(Settings::parse(&updated).table, "de");
        let fresh = Settings::default().update(HEADER);
        assert!(
            fresh.ends_with(
                "on = true\ndriver = \"auto\"\ndevice = \"auto\"\ntable = \"en-nabcc\"\n"
            )
        );
    }

    /// A USB device in a directory like the kernel's.
    fn device(root: &Path, name: &str, ids: (&str, &str), tty: Option<&str>) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("idVendor"), format!("{}\n", ids.0)).unwrap();
        std::fs::write(dir.join("idProduct"), format!("{}\n", ids.1)).unwrap();
        let interface = root.join(format!("{name}:1.0"));
        std::fs::create_dir_all(&interface).unwrap();
        if let Some(tty) = tty {
            std::fs::create_dir_all(interface.join(tty)).unwrap();
        }
    }

    #[test]
    fn a_display_is_found_by_its_ids_with_its_terminal() {
        let root = std::env::temp_dir().join(format!("braille-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("usb1")).unwrap();
        // A keyboard, and FTDI's own chip: not displays.
        device(&root, "1-1", ("046d", "c31c"), None);
        device(&root, "1-2", ("0403", "6001"), Some("ttyUSB0"));
        assert_eq!(find(&root), None);
        // A SuperVario 40, its port ttyUSB1.
        device(&root, "1-3", ("0403", "fe72"), Some("ttyUSB1"));
        let found = find(&root).unwrap();
        assert_eq!(
            found,
            Found { driver: "bm", name: "Baum SuperVario 40", device: "/devices/ttyUSB1".into() }
        );
        // A Brailliant, by CDC-ACM: its terminal is in `tty`.
        let _ = std::fs::remove_dir_all(root.join("1-3"));
        let _ = std::fs::remove_dir_all(root.join("1-3:1.0"));
        device(&root, "2-1", ("1c71", "c005"), Some("tty/ttyACM0"));
        assert_eq!(find(&root).unwrap().device, "/devices/ttyACM0");
        // Known, but with no terminal yet: not found.
        let _ = std::fs::remove_dir_all(root.join("2-1:1.0/tty"));
        assert_eq!(find(&root), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn brltty_runs_as_the_settings_and_the_display_found_say() {
        let found =
            Found { driver: "bm", name: "Baum SuperVario 40", device: "/devices/ttyUSB0".into() };
        let settings = Settings::default();
        let start = wanted(&settings, Some(&found)).unwrap().unwrap();
        assert_eq!(
            start.arguments(true).join(" "),
            "-n -e -b bm -d serial:/devices/ttyUSB0 -x dk -t en-nabcc"
        );
        // Without a configuration file of the person's, an empty one.
        assert!(start.arguments(false).join(" ").ends_with(" -t en-nabcc -f /devices/null"));
        assert_eq!(wanted(&settings, None), Ok(None));
        // Off: nothing, display or not.
        let off = Settings { on: false, ..Settings::default() };
        assert_eq!(wanted(&off, Some(&found)), Ok(None));
        // A driver named is used for a display found.
        let named = Settings { driver: "ht".into(), ..Settings::default() };
        assert_eq!(wanted(&named, Some(&found)).unwrap().unwrap().driver, "ht");
        // A port named wants its driver too, and needs no display found.
        let port = Settings { device: "/devices/ttyS0".into(), ..Settings::default() };
        assert!(wanted(&port, None).is_err());
        let port = Settings { driver: "al".into(), ..port };
        let start = wanted(&port, None).unwrap().unwrap();
        assert_eq!((start.driver.as_str(), start.device.as_str()), ("al", "/devices/ttyS0"));
    }

    #[test]
    fn every_display_has_one_of_brlttys_drivers_and_none_is_listed_twice() {
        for (i, (vendor, product, driver, _)) in DISPLAYS.iter().enumerate() {
            assert!(DRIVERS.contains(driver), "{driver}");
            assert!(!DISPLAYS[..i].iter().any(|d| (d.0, d.1) == (*vendor, *product)));
        }
    }
}
