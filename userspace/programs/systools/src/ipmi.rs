// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The IPMI commands bmc(8) sends the machine's baseboard management
//! controller, and the decoding of its answers, from the IPMI 2.0
//! specification (revision 1.1, 2019) and the Platform Management FRU
//! Information Storage Definition 1.0.
//!
//! Everything goes through [`Transport`], one request and its response, so
//! that the tests answer as a simulated BMC (`sim`) and `transport` is the
//! only file that knows Linux's `/dev/ipmi0` ABI, which Radix serves at
//! `/devices/ipmi0`. One module for each area: `device` (Get Device ID and
//! the system GUID), `sdr` and `sensor` (the sensor data repository, and
//! a sensor's reading and its conversion), `sel` and `events` (the system
//! event log and the text of an event), `fru` and `text` (the inventory
//! and its type/length strings), `lan` (the first LAN channel's settings)
//! and `chassis` (the identify light). `measure` sorts the numeric sensors
//! by what they measure, for `hardware sensors` and metricsd(8).
//!
//! Where to look when it misbehaves: `bmc sensors --json` shows each
//! sensor's raw byte beside its value, and a completion code the BMC
//! answers is named in the message with its number, which the
//! specification's table 5-2 lists.

mod chassis;
mod device;
mod events;
mod fru;
mod lan;
mod measure;
mod sdr;
mod sel;
mod sensor;
#[cfg(test)]
pub(crate) mod sim;
mod text;
mod transport;

use std::fmt;
use std::io;

pub(crate) use chassis::{Identify, identify};
pub(crate) use device::{DeviceId, device_id, system_guid};
pub(crate) use events::Severity;
pub(crate) use fru::{Fru, read_fru};
pub(crate) use lan::{Lan, find_lan_channel, read_lan, set_dhcp, set_static};
#[cfg(test)]
pub(crate) use measure::Status;
pub(crate) use measure::{Kind, Measured, measure};
pub(crate) use sdr::{Changed, entity_name, repository_changed, sensors};
pub(crate) use sel::{Entry, When, clear_log, info as log_info, read_log};
pub(crate) use sensor::{Reading, Sensor, read_sensor, type_name as sensor_type_name};
pub(crate) use transport::Device;

/// The network function of the chassis commands (table 5-1).
const NETFN_CHASSIS: u8 = 0x00;
/// The network function of the sensor and event commands.
const NETFN_SENSOR: u8 = 0x04;
/// The network function of the application commands.
const NETFN_APP: u8 = 0x06;
/// The network function of the storage commands: SDR, SEL and FRU.
const NETFN_STORAGE: u8 = 0x0A;
/// The network function of the transport commands: the LAN settings.
const NETFN_TRANSPORT: u8 = 0x0C;

/// One request to the BMC and its response.
pub(crate) trait Transport {
    /// Sends command `cmd` of network function `netfn` with `data` and
    /// gives the response, which starts with the completion code.
    ///
    /// # Errors
    ///
    /// If the request cannot be sent or no response comes.
    fn request(&mut self, netfn: u8, cmd: u8, data: &[u8]) -> io::Result<Vec<u8>>;
}

/// Why a command to the BMC failed.
#[derive(Debug)]
pub(crate) enum Error {
    /// The request could not be sent, or no response came.
    Transport(io::Error),
    /// The BMC answered with a completion code other than 0.
    Completion(u8),
    /// The response was shorter than the command's answer is, or held a
    /// value the specification does not allow; names what was read.
    Malformed(&'static str),
    /// Something a person asked for that the BMC cannot do, in words.
    Refused(String),
}

impl fmt::Display for Error {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Transport(error) => write!(out, "{}", crate::util::strip_os_error(error)),
            Error::Completion(code) => {
                write!(
                    out,
                    "the BMC refused the command: {} (0x{code:02X})",
                    completion_text(*code)
                )
            }
            Error::Malformed(what) => write!(out, "the BMC's answer is not valid: {what}"),
            Error::Refused(why) => write!(out, "{why}"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Transport(error)
    }
}

/// What a command of the BMC gives, or why it did not.
pub(crate) type Result<T> = std::result::Result<T, Error>;

/// The words of a completion code, from table 5-2 of the specification.
/// Codes 0x80 to 0xBE belong to the command that gave them.
fn completion_text(code: u8) -> &'static str {
    match code {
        0xC0 => "node busy",
        0xC1 => "invalid command",
        0xC2 => "invalid command for this LUN",
        0xC3 => "timeout while processing the command",
        0xC4 => "out of space",
        0xC5 => "reservation cancelled or invalid",
        0xC6 => "request data truncated",
        0xC7 => "request data length invalid",
        0xC8 => "request data field length limit exceeded",
        0xC9 => "parameter out of range",
        0xCA => "cannot return the number of requested data bytes",
        0xCB => "requested sensor, data or record not present",
        0xCC => "invalid data field in request",
        0xCD => "command illegal for the sensor or record type",
        0xCE => "command response could not be provided",
        0xCF => "cannot execute a duplicated request",
        0xD0 => "response not provided, SDR repository in update mode",
        0xD1 => "response not provided, device in firmware update mode",
        0xD2 => "response not provided, BMC initialization in progress",
        0xD3 => "destination unavailable",
        0xD4 => "insufficient privilege level",
        0xD5 => "command not supported in the present state",
        0xD6 => "parameter is illegal",
        0x80..=0xBE => "command-specific error",
        _ => "unspecified error",
    }
}

/// Sends a command and gives the data after its completion code, or the
/// code as an error.
fn command(link: &mut dyn Transport, netfn: u8, cmd: u8, data: &[u8]) -> Result<Vec<u8>> {
    let mut reply = link.request(netfn, cmd, data)?;
    let Some(&code) = reply.first() else {
        return Err(Error::Malformed("no completion code"));
    };
    if code != 0 {
        return Err(Error::Completion(code));
    }
    reply.remove(0);
    Ok(reply)
}

/// A 16-bit value the specification sends least significant byte first.
fn le16(bytes: &[u8], at: usize) -> Result<u16> {
    match bytes.get(at..at + 2) {
        Some(&[low, high]) => Ok(u16::from_le_bytes([low, high])),
        _ => Err(Error::Malformed("a 16-bit value is cut short")),
    }
}
