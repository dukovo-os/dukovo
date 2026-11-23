// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The baseboard management controller (BMC) of a server (Phase 19, ADR
//! 0661), through its keyboard controller style (KCS) interface: the
//! microcontroller that reads the machine's sensors, keeps its event log
//! and inventory, and runs a watchdog that restarts it, and that the
//! host asks through IPMI messages.
//!
//! A KCS interface is two registers in I/O port space: the data register
//! at the base address and the command and status register `spacing`
//! ports above it (1, 4 or 16; [`locate`] reads where firmware says
//! they are). The host sends a message one byte at a time, with the BMC
//! saying after each byte that it took it, then reads the response the
//! same way. The BMC's state, in the status register, says what it
//! expects next. The steps are those of section 9 of the IPMI 2.0
//! specification (see [`kcs`]).
//!
//! A message is a network function and a logical unit number, a command,
//! and its data; the response repeats them, with the network function
//! one higher, and has a completion code before its data. [`Bmc::request`]
//! sends one and checks the answer, and [`Bmc::probe`] asks Get Device ID
//! (section 20.1) to see that a BMC is there.
//!
//! The crate has no `unsafe` code: the kernel reaches the ports for it
//! ([`Platform`]). Its tests run it against a simulated BMC.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

mod kcs;
pub mod locate;
#[cfg(test)]
mod tests;

use core::fmt;

pub use locate::Interface;

/// What the BMC's interface needs from the kernel.
pub trait Platform {
    /// Reads an I/O port.
    fn in8(&self, port: u16) -> u8;
    /// Writes an I/O port.
    fn out8(&self, port: u16, value: u8);
    /// Waits at least `us` microseconds; it may sleep.
    fn wait_us(&self, us: u64);
}

/// The most bytes in a message in either direction, the network function
/// and command included. The specification leaves the KCS maximum to the
/// implementation; the BMCs the kernel meets take at least this much.
pub const MAX_MESSAGE: usize = 272;

/// The network function of the application commands (section 5.1,
/// table 5-1), the group Get Device ID is in.
pub const NETFN_APP: u8 = 0x06;
/// Get Device ID (section 20.1, table 20-2).
pub const CMD_GET_DEVICE_ID: u8 = 0x01;

/// Why a message did not get through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The BMC stayed busy, or did not answer, for the time a message
    /// may take.
    Timeout,
    /// The status register reads all ones: no BMC answers at these ports.
    NoDevice,
    /// The BMC left the order of steps after three tries.
    Interface,
    /// The network function is not one a request has.
    BadNetfn(u8),
    /// The request does not fit in a message.
    TooBig,
    /// The response is shorter than a network function, a command and a
    /// completion code.
    ShortResponse,
    /// The response answers another command than the one sent.
    WrongResponse,
    /// The response has more bytes than the buffer for it.
    ResponseTooBig(usize),
    /// The BMC's completion code, not success.
    Completion(u8),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Timeout => f.write_str("the BMC stayed busy"),
            Error::NoDevice => f.write_str("no BMC answers at the interface's ports"),
            Error::Interface => f.write_str("the BMC's KCS interface did not follow its protocol"),
            Error::BadNetfn(netfn) => write!(f, "network function {netfn:#x} is not a request's"),
            Error::TooBig => f.write_str("the request is too big"),
            Error::ShortResponse => f.write_str("the BMC's response is too short"),
            Error::WrongResponse => f.write_str("the BMC's response is for another command"),
            Error::ResponseTooBig(len) => {
                write!(f, "the BMC's response of {len} bytes does not fit")
            }
            Error::Completion(0xc1) => f.write_str("the BMC does not take the command"),
            Error::Completion(0xc3) => f.write_str("the BMC timed out on the command"),
            Error::Completion(0xff) => f.write_str("the BMC failed the command"),
            Error::Completion(code) => write!(f, "the BMC answered completion code {code:#x}"),
        }
    }
}

/// What Get Device ID says (section 20.1, table 20-2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceId {
    /// The device's number, from its maker.
    pub device_id: u8,
    /// Its revision: bits 3:0 of the byte.
    pub revision: u8,
    /// Whether it provides device SDRs (bit 7 of the revision byte).
    pub provides_sdrs: bool,
    /// Its firmware's major revision, in binary.
    pub firmware_major: u8,
    /// Its firmware's minor revision: two BCD digits, as a number.
    pub firmware_minor: u8,
    /// Whether the firmware is in the middle of an update, or the BMC
    /// otherwise not able to take commands (bit 7 of the major byte).
    pub updating: bool,
    /// The IPMI version it follows: major, then minor, from BCD.
    pub ipmi_version: (u8, u8),
    /// The IANA enterprise number of its maker.
    pub manufacturer: u32,
    /// The maker's number for the product.
    pub product: u16,
}

impl DeviceId {
    /// The bytes of the response after its completion code: the first 11
    /// are required; the auxiliary firmware revision after them is not
    /// read.
    pub fn parse(data: &[u8]) -> Option<DeviceId> {
        let &[id, revision, major, minor, version, _, m0, m1, m2, p0, p1, ..] = data else {
            return None;
        };
        Some(DeviceId {
            device_id: id,
            revision: revision & 0x0f,
            provides_sdrs: revision & 0x80 != 0,
            firmware_major: major & 0x7f,
            firmware_minor: (minor >> 4) * 10 + (minor & 0x0f),
            updating: major & 0x80 != 0,
            // The low nibble is the major version, the high one the minor.
            ipmi_version: (version & 0x0f, version >> 4),
            manufacturer: u32::from_le_bytes([m0, m1, m2, 0]),
            product: u16::from_le_bytes([p0, p1]),
        })
    }
}

/// The name of the IANA enterprise number `number`, if it is one of the
/// makers of servers the kernel names.
pub fn manufacturer_name(number: u32) -> Option<&'static str> {
    Some(match number {
        2 => "IBM",
        11 => "Hewlett-Packard",
        343 => "Intel",
        674 => "Dell",
        2011 => "Huawei",
        10368 => "Fujitsu",
        10876 => "Supermicro",
        19046 => "Lenovo",
        _ => return None,
    })
}

/// A BMC, as reached through one KCS interface.
pub struct Bmc<P> {
    platform: P,
    interface: Interface,
}

impl<P: Platform> Bmc<P> {
    /// The BMC behind `interface`. Nothing is sent until a request.
    pub fn new(platform: P, interface: Interface) -> Bmc<P> {
        Bmc { platform, interface }
    }

    /// Where the interface is.
    pub fn interface(&self) -> Interface {
        self.interface
    }

    /// The platform, for a caller that changes how it waits.
    pub fn platform_mut(&mut self) -> &mut P {
        &mut self.platform
    }

    /// Asks the BMC for its Get Device ID, to see that one answers.
    ///
    /// # Errors
    ///
    /// Any error of [`Bmc::request`], [`Error::Completion`] if the BMC
    /// does not give success, and [`Error::ShortResponse`] if the answer
    /// lacks fields.
    pub fn probe(&mut self) -> Result<DeviceId, Error> {
        let mut response = [0; 32];
        let len = self.request(NETFN_APP, CMD_GET_DEVICE_ID, &[], &mut response)?;
        match response[..len] {
            [0, ref data @ ..] => DeviceId::parse(data).ok_or(Error::ShortResponse),
            [code, ..] => Err(Error::Completion(code)),
            [] => Err(Error::ShortResponse),
        }
    }

    /// Sends command `cmd` of network function `netfn` (a request's, so
    /// even) to logical unit 0 with `data`, and puts the response in
    /// `response`: its completion code, then its data. Returns how many
    /// bytes that is. A completion code that is not success is not an
    /// error here, since the BMC's data after it can still mean
    /// something; the caller reads the first byte.
    ///
    /// # Errors
    ///
    /// [`Error::Timeout`] if the exchange takes over 5 seconds,
    /// [`Error::NoDevice`], [`Error::Interface`] after three tries,
    /// [`Error::WrongResponse`] if the BMC answers another command,
    /// and the errors for a request or response that does not fit.
    pub fn request(
        &mut self,
        netfn: u8,
        cmd: u8,
        data: &[u8],
        response: &mut [u8],
    ) -> Result<usize, Error> {
        // The response's network function is the next one, in 6 bits.
        if netfn > 0x3e || netfn & 1 != 0 {
            return Err(Error::BadNetfn(netfn));
        }
        if data.len() + 2 > MAX_MESSAGE {
            return Err(Error::TooBig);
        }
        let mut kcs = kcs::Kcs::new(&self.platform, self.interface);
        let (header, total) = kcs.exchange([netfn << 2, cmd], data, response)?;
        let len = total.checked_sub(2).filter(|&len| len > 0).ok_or(Error::ShortResponse)?;
        if header[0] >> 2 != netfn + 1 || header[1] != cmd {
            return Err(Error::WrongResponse);
        }
        if len > response.len() {
            return Err(Error::ResponseTooBig(len));
        }
        Ok(len)
    }
}
