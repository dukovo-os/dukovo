// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! bmc(8): what the machine's baseboard management controller says, over
//! the kernel's IPMI interface at `/devices/ipmi0`: its sensors, its event
//! log, its inventory, its network settings, and the identify light.
//!
//! The commands are in the `ipmi` module; `views` holds the text and the
//! JSON of each answer. Where to look when it misbehaves: `bmc --json`
//! shows what the BMC says of itself, and the kernel's log has the line
//! that says which interface it found ("IPMI: BMC ...").

mod views;

use std::ffi::OsString;
use std::net::Ipv4Addr;

use crate::args::{Arg, Args};
use crate::ipmi::{self, Identify, Transport};
use crate::util::{Fail, fail, usage};
use views::{
    EventList, EventView, FruView, IdentifyView, LanView, Report, SensorList, SensorView, Summary,
};

const USAGE: &str = "usage: bmc [--json]\n       bmc sensors [--json]\n       bmc events \
                     [--clear] [--json]\n       bmc fru [--json]\n       bmc identify \
                     [SECONDS | on | off] [--json]\n       bmc lan [--json]\n       bmc lan set \
                     address ADDRESS/PREFIX [gateway GATEWAY] [--json]\n       bmc lan set dhcp \
                     [--json]";

/// Seconds the identify light is on when none are given, as the
/// specification's default.
const IDENTIFY_DEFAULT: u8 = 15;

/// What to do.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Summary,
    Sensors,
    Events { clear: bool },
    Fru,
    Identify(Identify),
    Lan,
    LanStatic { address: Ipv4Addr, mask: Ipv4Addr, gateway: Option<Ipv4Addr> },
    LanDhcp,
}

/// The command line.
#[derive(Debug, PartialEq, Eq)]
struct Options {
    command: Command,
    json: bool,
}

/// bmc(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let options = parse(args)?;
    let mut device = open(&options.command)?;
    let report = run(&mut device, &options.command)?;
    if options.json {
        println!("{}", report.json()?);
    } else {
        print!("{}", report.text());
    }
    Ok(0)
}

/// Opens the BMC's interface, or says why the machine has none for this
/// program.
fn open(command: &Command) -> Result<ipmi::Device, Fail> {
    ipmi::Device::open().map_err(|error| open_failure(&error, command))
}

fn open_failure(error: &std::io::Error, command: &Command) -> Fail {
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            fail("this machine has no BMC that Dukovo reaches".to_string())
        }
        std::io::ErrorKind::PermissionDenied => {
            fail(format!("the BMC is for an administrator: run elevate bmc {}", command.word()))
        }
        _ => fail(format!("cannot open /devices/ipmi0: {}", crate::util::strip_os_error(error))),
    }
}

impl Command {
    /// The words of the command, for the advice to run it elevated.
    fn word(&self) -> &'static str {
        match self {
            Command::Summary => "",
            Command::Sensors => "sensors",
            Command::Events { clear: true } => "events --clear",
            Command::Events { clear: false } => "events",
            Command::Fru => "fru",
            Command::Identify(_) => "identify",
            Command::Lan => "lan",
            Command::LanStatic { .. } | Command::LanDhcp => "lan set",
        }
    }
}

fn parse(args: &mut Args) -> Result<Options, Fail> {
    let mut json = false;
    let mut clear = false;
    let mut words: Vec<String> = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => json = true,
            Arg::Long(name) if name == "clear" => clear = true,
            Arg::Operand(word) => words.push(text(word)?),
            other => return Err(args.unknown(&other)),
        }
    }
    let mut words = words.into_iter();
    let name = words.next();
    let rest: Vec<String> = words.collect();
    if clear && name.as_deref() != Some("events") {
        return Err(usage("--clear is for bmc events".into()));
    }
    let command = match name.as_deref() {
        None => no_words(&rest, Command::Summary)?,
        Some("sensors") => no_words(&rest, Command::Sensors)?,
        Some("events") => no_words(&rest, Command::Events { clear })?,
        Some("fru") => no_words(&rest, Command::Fru)?,
        Some("identify") => identify_command(&rest)?,
        Some("lan") => lan_command(&rest)?,
        Some(other) => {
            return Err(usage(format!("no command named {other}\n{USAGE}")));
        }
    };
    Ok(Options { command, json })
}

fn text(word: OsString) -> Result<String, Fail> {
    word.into_string().map_err(|word| usage(format!("'{}' is not UTF-8", word.to_string_lossy())))
}

/// `command`, for a command that takes no operands.
fn no_words(rest: &[String], command: Command) -> Result<Command, Fail> {
    match rest.first() {
        None => Ok(command),
        Some(extra) => Err(usage(format!("unexpected argument '{extra}'"))),
    }
}

fn identify_command(rest: &[String]) -> Result<Command, Fail> {
    let what = match rest {
        [] => Identify::Seconds(IDENTIFY_DEFAULT),
        [word] if word == "on" => Identify::ForceOn,
        [word] if word == "off" => Identify::Off,
        [word] => match word.parse::<u8>() {
            Ok(seconds @ 1..=255) => Identify::Seconds(seconds),
            _ => {
                return Err(usage(format!(
                    "'{word}' is not a number of seconds from 1 to 255, on or off"
                )));
            }
        },
        [_, extra, ..] => return Err(usage(format!("unexpected argument '{extra}'"))),
    };
    Ok(Command::Identify(what))
}

fn lan_command(rest: &[String]) -> Result<Command, Fail> {
    let words: Vec<&str> = rest.iter().map(String::as_str).collect();
    match words[..] {
        [] => Ok(Command::Lan),
        ["set", "dhcp"] => Ok(Command::LanDhcp),
        ["set", "address", cidr] => static_command(cidr, None),
        ["set", "address", cidr, "gateway", gateway] => {
            let gateway = gateway
                .parse::<Ipv4Addr>()
                .map_err(|_| usage(format!("'{gateway}' is not an IPv4 address")))?;
            static_command(cidr, Some(gateway))
        }
        _ => Err(usage(format!("bmc lan set takes address ADDRESS/PREFIX or dhcp\n{USAGE}"))),
    }
}

/// `set address A/PREFIX` as a command.
fn static_command(cidr: &str, gateway: Option<Ipv4Addr>) -> Result<Command, Fail> {
    let bad =
        || usage(format!("'{cidr}' is not an IPv4 address and prefix length, as 10.0.0.5/24"));
    let (address, prefix) = cidr.split_once('/').ok_or_else(bad)?;
    let address: Ipv4Addr = address.parse().map_err(|_| bad())?;
    let prefix: u32 = prefix.parse().map_err(|_| bad())?;
    if !(1..=32).contains(&prefix) || address.is_unspecified() || address.is_multicast() {
        return Err(bad());
    }
    let mask = Ipv4Addr::from(u32::MAX << (32 - prefix));
    Ok(Command::LanStatic { address, mask, gateway })
}

/// Runs `command` on the BMC `link` talks to.
fn run(link: &mut dyn Transport, command: &Command) -> Result<Report, Fail> {
    run_ipmi(link, command).map_err(|error| fail(error.to_string()))
}

fn run_ipmi(link: &mut dyn Transport, command: &Command) -> Result<Report, ipmi::Error> {
    Ok(match command {
        Command::Summary => {
            let id = ipmi::device_id(link)?;
            let guid = ipmi::system_guid(link)?;
            let lan = match ipmi::find_lan_channel(link)? {
                Some(channel) => Some(ipmi::read_lan(link, channel)?),
                None => None,
            };
            Report::Summary(Summary::new(&id, guid, lan.as_ref()))
        }
        Command::Sensors => Report::Sensors(sensor_list(link)?),
        Command::Events { clear } => Report::Events(event_list(link, *clear)?),
        Command::Fru => Report::Fru(Box::new(FruView::new(ipmi::read_fru(link, 0)?))),
        Command::Identify(what) => {
            ipmi::identify(link, *what)?;
            Report::Identify(IdentifyView::new(*what))
        }
        Command::Lan => Report::Lan(LanView::new(&read_first_lan(link)?)),
        Command::LanStatic { address, mask, gateway } => {
            let channel = lan_channel(link)?;
            ipmi::set_static(link, channel, *address, *mask, *gateway)?;
            Report::Lan(LanView::new(&ipmi::read_lan(link, channel)?))
        }
        Command::LanDhcp => {
            let channel = lan_channel(link)?;
            ipmi::set_dhcp(link, channel)?;
            Report::Lan(LanView::new(&ipmi::read_lan(link, channel)?))
        }
    })
}

fn lan_channel(link: &mut dyn Transport) -> Result<u8, ipmi::Error> {
    ipmi::find_lan_channel(link)?
        .ok_or_else(|| ipmi::Error::Refused("this BMC has no LAN channel".into()))
}

fn read_first_lan(link: &mut dyn Transport) -> Result<ipmi::Lan, ipmi::Error> {
    let channel = lan_channel(link)?;
    ipmi::read_lan(link, channel)
}

fn sensor_list(link: &mut dyn Transport) -> Result<SensorList, ipmi::Error> {
    let mut views = Vec::new();
    for sensor in ipmi::sensors(link)? {
        let reading =
            if sensor.readable_here() { Some(ipmi::read_sensor(link, &sensor)?) } else { None };
        views.push(SensorView::new(link, &sensor, reading.as_ref())?);
    }
    Ok(SensorList { sensors: views })
}

/// The log, with the sensors' names from the repository when it can be
/// read; a BMC with no repository still has its log.
fn event_list(link: &mut dyn Transport, clear: bool) -> Result<EventList, ipmi::Error> {
    let entries = ipmi::read_log(link)?;
    let sensors = ipmi::sensors(link).unwrap_or_default();
    let mut events = Vec::new();
    for entry in &entries {
        events.push(EventView::new(link, entry, &sensors)?);
    }
    if clear {
        ipmi::clear_log(link)?;
    }
    Ok(EventList { events, cleared: clear })
}

#[cfg(test)]
mod tests;
