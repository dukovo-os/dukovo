// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! guest-agent(8): answers the host of a virtual machine in QEMU's guest
//! agent protocol (ADR 0329), as qemu-ga does on Linux, so that Proxmox,
//! libvirt and OpenStack show the machine's addresses and system and can
//! shut it down cleanly.
//!
//! The host talks over the virtio console port QEMU names
//! `org.qemu.guest_agent.0`, which the kernel links from
//! `/devices/virtio-ports/` (ADR 0327). Each request is a JSON object,
//! `{"execute": "guest-ping"}`, perhaps with `"arguments"` and an `"id"`
//! to echo; each answer is one line of JSON, `{"return": ...}` or
//! `{"error": {"class": ..., "desc": ...}}`. Requests follow one another
//! on the stream with no delimiter of their own, so [`Splitter`] cuts
//! them out by their braces; a 0xFF byte, which no JSON holds, throws away
//! what came before it, as `guest-sync-delimited`'s clients send to begin
//! afresh. [`answer`] says what each command answers.
//!
//! On a machine without a virtio console, the service ends at once:
//! there is nothing to answer. On one with a console, it waits up to
//! [`PORT_WAIT`] for the port: the kernel links it only once the host has
//! named it, which can come after the service starts.
//!
//! If the host gets no answer, look for the `guest-agent:` lines of the
//! log, then try the port by hand from the host (guest-agent(8)).

#[cfg(test)]
mod tests;

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::args::{Arg, Args};
use crate::util::{self, Fail};
use crate::{ip, lspci, power};

/// The port QEMU's guest agent channel is, by its name.
pub const PORT: &str = "/devices/virtio-ports/org.qemu.guest_agent.0";

/// A virtio console's PCI vendor and device, modern and transitional
/// (virtio 1.2, section 4.1.2.1).
const VIRTIO_CONSOLE: [(&str, &str); 2] = [("0x1af4", "0x1043"), ("0x1af4", "0x1003")];

/// How long it waits for the host to name its port, on a machine with a
/// virtio console.
const PORT_WAIT: Duration = Duration::from_secs(10);

/// The most bytes a request may hold: more is thrown away and refused.
const MOST: usize = 64 * 1024;

/// The byte that throws away what came before it: never part of JSON,
/// and sent before `guest-sync-delimited`'s answer too.
const RESET: u8 = 0xFF;

/// The agent's version, given by `guest-info`.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commands it answers, in `guest-info`'s order, and whether each
/// answers when it succeeds: `guest-shutdown` does not, as qemu-ga's
/// does not.
const COMMANDS: [(&str, bool); 9] = [
    ("guest-sync-delimited", true),
    ("guest-sync", true),
    ("guest-ping", true),
    ("guest-info", true),
    ("guest-get-osinfo", true),
    ("guest-get-host-name", true),
    ("guest-network-get-interfaces", true),
    ("guest-get-time", true),
    ("guest-shutdown", false),
];

/// Writes a line of the log, `guest-agent:` first.
macro_rules! say {
    ($($arg:tt)*) => {
        println!("guest-agent: {}", format_args!($($arg)*))
    };
}

/// The program: `guest-agent [--port PATH]`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut port = PathBuf::from(PORT);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(l) if l == "port" => port = PathBuf::from(args.value()?),
            Arg::Short('h') => return help(),
            Arg::Long(l) if l == "help" => return help(),
            other => return Err(args.unknown(&other)),
        }
    }
    if !port.exists() {
        if !has_virtio_console() {
            say!("no guest agent channel, {}: nothing to answer", port.display());
            return Ok(0);
        }
        if !appears(&port, PORT_WAIT) {
            say!(
                "no guest agent channel, {}: the host named no such port within {} s ({}): \
                 nothing to answer",
                port.display(),
                PORT_WAIT.as_secs(),
                named_ports(port.parent().unwrap_or(Path::new("/")))
            );
            return Ok(0);
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&port)
        .map_err(|err| util::failed(&port.display(), err))?;
    say!("answering the host on {}", port.display());
    serve(&file, &port)
}

/// Whether the machine has a virtio console, among the PCI functions the
/// kernel found.
fn has_virtio_console() -> bool {
    let Ok(functions) = std::fs::read_dir(lspci::DEVICES) else { return false };
    functions.flatten().any(|function| {
        let read = |name| std::fs::read_to_string(function.path().join(name)).unwrap_or_default();
        let (vendor, device) = (read("vendor"), read("device"));
        VIRTIO_CONSOLE.iter().any(|&ids| ids == (vendor.trim(), device.trim()))
    })
}

/// Whether `path` is there within `wait`, looking every 50 ms.
fn appears(path: &Path, wait: Duration) -> bool {
    let until = Instant::now() + wait;
    loop {
        if path.exists() {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The ports the host has named, linked in `dir`, for the line that says
/// the agent's is not among them: "it named none", or "it named a and b".
fn named_ports(dir: &Path) -> String {
    let mut names: Vec<String> = std::fs::read_dir(dir).map_or_default(|entries| {
        entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect()
    });
    names.sort();
    match names.split_last() {
        None => String::from("it named none"),
        Some((last, [])) => format!("it named {last}"),
        Some((last, rest)) => format!("it named {} and {last}", rest.join(", ")),
    }
}

fn help() -> Result<i32, Fail> {
    println!(
        "Usage: guest-agent [--port PATH]\n\n\
         Answers the virtual machine's host in QEMU's guest agent protocol on\n\
         {PORT}, or PATH.\n\
         See guest-agent(8)."
    );
    Ok(0)
}

/// Answers the requests that come on `port` until it goes; `path` names
/// it in the log.
fn serve(port: &File, path: &Path) -> Result<i32, Fail> {
    let mut splitter = Splitter::default();
    let mut buffer = [0u8; 4096];
    loop {
        let read = match (&*port).read(&mut buffer) {
            // No host holds the other end: wait for one.
            Ok(0) => {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            // The port was taken away.
            Err(err) => return Err(util::failed(&path.display(), err)),
        };
        for piece in splitter.feed(&buffer[..read]) {
            let (text, end) = match piece {
                Piece::Request(text) => answer(&text, &System),
                Piece::TooLong => {
                    (Some(error("GenericError", "the request is longer than 64 KiB")), None)
                }
            };
            if let Some(text) = text {
                (&*port).write_all(&text).map_err(|err| util::failed(&path.display(), err))?;
            }
            if let Some(end) = end {
                let words = match end {
                    "poweroff" => "power the machine off",
                    "reboot" => "restart the machine",
                    _ => "halt the machine",
                };
                say!("the host asked to {words}");
                match power::request(end) {
                    Ok(answer) => {
                        if let Some(why) = answer.strip_prefix("error: ") {
                            say!("Spark refused: {}", why.trim_end());
                        }
                    }
                    Err(fail) => say!("Spark did not take it: {}", fail.message),
                }
            }
        }
    }
}

/// What [`Splitter`] cut out of the stream.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece {
    /// A request's text, from its `{` to its `}`, or a stray word between
    /// requests, which [`answer`] refuses as JSON.
    Request(Vec<u8>),
    /// A request longer than [`MOST`], thrown away.
    TooLong,
}

/// Cuts requests out of the stream as they come, by their braces: a
/// request ends at the `}` that closes its first `{`, braces in strings
/// not counted.
#[derive(Debug, Default)]
pub struct Splitter {
    /// The request so far.
    text: Vec<u8>,
    /// How many braces and brackets are open.
    depth: usize,
    /// Within a string, and after its backslash.
    in_string: bool,
    escaped: bool,
    /// Throwing a request away until its end, as it is too long.
    skipping: bool,
}

impl Splitter {
    /// The pieces `bytes` ends, in order.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Piece> {
        let mut pieces = Vec::new();
        for &byte in bytes {
            if byte == RESET {
                *self = Splitter::default();
                continue;
            }
            if self.depth == 0 && self.text.is_empty() {
                if byte.is_ascii_whitespace() {
                    continue;
                }
                self.skipping = false;
            }
            if !self.skipping {
                self.text.push(byte);
            }
            if self.in_string {
                match (self.escaped, byte) {
                    (true, _) => self.escaped = false,
                    (false, b'\\') => self.escaped = true,
                    (false, b'"') => self.in_string = false,
                    _ => {}
                }
            } else {
                match byte {
                    b'"' => self.in_string = true,
                    b'{' | b'[' => self.depth += 1,
                    b'}' | b']' => self.depth = self.depth.saturating_sub(1),
                    _ => {}
                }
            }
            // A stray word between requests ends at a space or a newline.
            let stray_end = self.depth == 0 && !self.in_string && byte.is_ascii_whitespace();
            let closed = self.depth == 0 && !self.in_string && matches!(byte, b'}' | b']');
            if closed || stray_end {
                if self.skipping {
                    pieces.push(Piece::TooLong);
                } else {
                    pieces.push(Piece::Request(std::mem::take(&mut self.text)));
                }
                self.text.clear();
                self.skipping = false;
            } else if self.text.len() > MOST {
                self.text = Vec::new();
                self.skipping = true;
            }
        }
        pieces
    }
}

/// What [`answer`] needs of the system, so that the tests can give their
/// own.
pub trait Facts {
    /// The machine's name.
    fn host_name(&self) -> String;
    /// The time, in nanoseconds since the epoch.
    fn time_ns(&self) -> u64;
    /// The system: its kernel's release and version, its machine, and the
    /// release's name and version.
    fn os(&self) -> Os;
    /// The network interfaces.
    fn interfaces(&self) -> Vec<ip::Interface>;
}

/// The system, as `guest-get-osinfo` gives it.
#[derive(Debug, Default)]
pub struct Os {
    pub kernel_release: String,
    pub kernel_version: String,
    pub machine: String,
    pub name: String,
    pub version: String,
}

/// The running system's facts.
struct System;

impl Facts for System {
    fn host_name(&self) -> String {
        util::hostname()
    }

    fn time_ns(&self) -> u64 {
        util::now_ns()
    }

    fn os(&self) -> Os {
        let (_, release, machine) = util::uname();
        // `Radix 0.14.0 (Dukovo Server 0.14.0)`.
        let version = std::fs::read_to_string("/kernel/version").unwrap_or_default();
        let (kernel, system) = version.trim().split_once(" (").unwrap_or((version.trim(), ""));
        let (name, release_version) =
            system.strip_suffix(')').and_then(|s| s.rsplit_once(' ')).unwrap_or(("Dukovo", ""));
        Os {
            kernel_release: release,
            kernel_version: kernel.to_string(),
            machine,
            name: name.to_string(),
            version: release_version.to_string(),
        }
    }

    fn interfaces(&self) -> Vec<ip::Interface> {
        ip::read_interfaces(Path::new(ip::INTERFACES), Path::new(ip::ROUTES)).unwrap_or_default()
    }
}

/// The answer to request `text`, one line of JSON, if it has one; and how
/// the machine is to end after it, for `guest-shutdown`.
pub fn answer(text: &[u8], facts: &dyn Facts) -> (Option<Vec<u8>>, Option<&'static str>) {
    let request: Map<String, Value> = match serde_json::from_slice(text) {
        Ok(Value::Object(request)) => request,
        Ok(_) => return (Some(error("GenericError", "a request is a JSON object")), None),
        Err(err) => {
            return (Some(error("GenericError", &format!("the request is not JSON: {err}"))), None);
        }
    };
    let id = request.get("id").cloned();
    let with_id = |mut answer: Value| {
        if let (Some(id), Value::Object(fields)) = (&id, &mut answer) {
            fields.insert("id".into(), id.clone());
        }
        line(&answer)
    };
    let Some(command) = request.get("execute").and_then(Value::as_str) else {
        return (
            Some(with_id(error_value("GenericError", "the request has no \"execute\""))),
            None,
        );
    };
    let empty = Map::new();
    let arguments = request.get("arguments").and_then(Value::as_object).unwrap_or(&empty);
    let returned = match command {
        "guest-sync" | "guest-sync-delimited" => match arguments.get("id") {
            Some(sync) if sync.is_i64() || sync.is_u64() => {
                let mut text = line(&json!({ "return": sync }));
                if command == "guest-sync-delimited" {
                    text.insert(0, RESET);
                }
                return (Some(text), None);
            }
            _ => Err(format!("{command} takes an integer \"id\"")),
        },
        "guest-ping" => Ok(json!({})),
        "guest-info" => Ok(json!({
            "version": VERSION,
            "supported_commands": COMMANDS.iter().map(|&(name, success)| json!({
                "name": name,
                "enabled": true,
                "success-response": success,
            })).collect::<Vec<_>>(),
        })),
        "guest-get-osinfo" => {
            let os = facts.os();
            Ok(json!({
                "id": "dukovo",
                "name": os.name,
                "pretty-name": format!("{} {}", os.name, os.version).trim().to_string(),
                "version": os.version,
                "version-id": os.version,
                "kernel-release": os.kernel_release,
                "kernel-version": os.kernel_version,
                "machine": os.machine,
            }))
        }
        "guest-get-host-name" => Ok(json!({ "host-name": facts.host_name() })),
        "guest-get-time" => Ok(json!(facts.time_ns())),
        "guest-network-get-interfaces" => {
            Ok(Value::Array(facts.interfaces().iter().map(interface).collect()))
        }
        "guest-shutdown" => {
            let mode = arguments.get("mode").and_then(Value::as_str).unwrap_or("powerdown");
            let end = match mode {
                "powerdown" => "poweroff",
                "reboot" => "reboot",
                "halt" => "halt",
                other => {
                    let why =
                        format!("guest-shutdown's mode is powerdown, reboot or halt, not {other}");
                    return (Some(with_id(error_value("GenericError", &why))), None);
                }
            };
            // No answer when it works, as qemu-ga gives none.
            return (None, Some(end));
        }
        other => {
            let why = format!("guest-agent does not answer {other}: guest-info lists what it does");
            return (Some(with_id(error_value("CommandNotFound", &why))), None);
        }
    };
    match returned {
        Ok(value) => (Some(with_id(json!({ "return": value }))), None),
        Err(why) => (Some(with_id(error_value("GenericError", &why))), None),
    }
}

/// An interface as `guest-network-get-interfaces` gives it: its name, its
/// hardware address, its addresses and its counts.
fn interface(interface: &ip::Interface) -> Value {
    let mut addresses: Vec<Value> = interface
        .addresses
        .iter()
        .map(|a| json!({ "ip-address-type": "ipv4", "ip-address": a.ip.to_string(), "prefix": a.prefix }))
        .collect();
    addresses.extend(interface.ipv6.addresses.iter().filter(|a| !a.tentative).map(|a| {
        json!({ "ip-address-type": "ipv6", "ip-address": a.cidr.ip.to_string(), "prefix": a.cidr.prefix })
    }));
    let [rx_bytes, rx_packets, rx_errs, rx_dropped] = interface.received;
    let [tx_bytes, tx_packets, tx_errs, tx_dropped] = interface.sent;
    let mac = if interface.mac.is_empty() { "00:00:00:00:00:00" } else { interface.mac.as_str() };
    json!({
        "name": interface.name,
        "hardware-address": mac,
        "ip-addresses": addresses,
        "statistics": {
            "rx-bytes": rx_bytes, "rx-packets": rx_packets, "rx-errs": rx_errs, "rx-dropped": rx_dropped,
            "tx-bytes": tx_bytes, "tx-packets": tx_packets, "tx-errs": tx_errs, "tx-dropped": tx_dropped,
        },
    })
}

/// An error answer of `class`, saying `why`.
fn error_value(class: &str, why: &str) -> Value {
    json!({ "error": { "class": class, "desc": why } })
}

/// [`error_value`] as a line.
fn error(class: &str, why: &str) -> Vec<u8> {
    line(&error_value(class, why))
}

/// `value` as one line of JSON.
fn line(value: &Value) -> Vec<u8> {
    let mut text = serde_json::to_vec(value).unwrap_or_default();
    text.push(b'\n');
    text
}
