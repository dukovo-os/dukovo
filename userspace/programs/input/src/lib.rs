// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Input devices for programs (release 1.1, ADR 0706): what a program
//! needs to read the machine's keyboards, mice and tablets through the
//! files of _/devices/input_ (radix-input(2)).
//!
//! - [`wire`] reads the interface's records from the bytes the kernel
//!   writes: the events of a device's file, the devices that came and
//!   went, and what a device says of itself.
//! - [`Device`] sends a device's file its requests, over whatever sends
//!   a `control` request to the program's description of the file
//!   ([`Control`]), and [`devices`] lists the devices there are.
//! - [`kind`] says what a device is, a keyboard, a pointer or a tablet,
//!   from the keys and the axes it has, so that every program calls a
//!   device the same.
//! - [`service`] is what inputd(8) and a program that holds the screens
//!   say to each other when the devices are handed out (ADR 0707).
//!
//! Nothing here opens a file or makes a system call of its own, so its
//! tests run on the build machine, where they check each record against
//! the kernel's own writer of it. A program implements [`Control`] with
//! radix-file(2)'s `control`, reads a device's events from the file
//! itself, and gives the bytes to [`wire::events`].

#![forbid(unsafe_code)]

pub mod kind;
pub mod service;
pub mod wire;

#[cfg(test)]
mod tests;

use std::io;

pub use radix_abi::input as abi;
use radix_abi::input::{
    EV_ABS, EV_KEY, EV_REL, EV_SW, INPUT_AXIS, INPUT_BITS, INPUT_INFO, INPUT_LIST, INPUT_NAME,
    INPUT_PLACE, INPUT_SERIAL, INPUT_STATE, INPUT_TEXT_MAX, INPUT_TIE, InputAxis, InputInfo,
    KEY_CODES,
};

use kind::{Has, Kinds};

/// How a request reaches a file of _/devices/input_.
pub trait Control {
    /// Sends the file `request` with `argument`, and `buffer` for the
    /// kernel to write its answer into, as radix-file(2)'s `control`
    /// does. Returns the request's result.
    ///
    /// # Errors
    ///
    /// The request's failure, with the error number the C library gives
    /// it: EBUSY for busy, EINVAL for invalid_argument.
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64>;
}

impl<C: Control + ?Sized> Control for &C {
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
        (**self).control(request, argument, buffer)
    }
}

/// The bytes of the longest answer INPUT_BITS and INPUT_STATE have: a
/// bit for each key's code.
pub const BITS_MAX: usize = KEY_CODES as usize / 8;

/// The kernel's answer was not the record it should be.
fn short() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "the input device's answer was cut short")
}

/// The number of each device there is, lowest first (INPUT_LIST), asked
/// of `control`, a description of _/devices/input/control_.
///
/// # Errors
///
/// The request's failure.
pub fn devices(control: &impl Control) -> io::Result<Vec<u32>> {
    let mut room = 16;
    loop {
        let mut bytes = vec![0; room * 4];
        let count = control.control(INPUT_LIST, 0, &mut bytes)? as usize;
        if count <= room {
            return Ok(wire::numbers(&bytes[..count * 4]).collect());
        }
        // More than fit, or some came since: again, with room.
        room = count;
    }
}

/// An input device: a description of its file, _/devices/input/N_, and
/// the requests on it.
#[derive(Debug)]
pub struct Device<C> {
    file: C,
}

impl<C: Control> Device<C> {
    /// The device whose file `file` reaches.
    pub fn new(file: C) -> Device<C> {
        Device { file }
    }

    /// What reaches its file.
    pub fn file(&self) -> &C {
        &self.file
    }

    /// Gives back what reaches its file.
    pub fn into_file(self) -> C {
        self.file
    }

    /// INPUT_INFO: its number, how it is connected, its maker's and its
    /// model's numbers and the rate it asks to be read at.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn info(&self) -> io::Result<InputInfo> {
        let mut bytes = [0; InputInfo::SIZE];
        self.file.control(INPUT_INFO, 0, &mut bytes)?;
        wire::info(&bytes).ok_or_else(short)
    }

    /// INPUT_BITS: a bit for each code of `kind` (an EV_*) the device
    /// has, without the zero bytes at the end.
    ///
    /// # Errors
    ///
    /// The request's failure: EINVAL for a kind there is not.
    pub fn bits(&self, kind: u16) -> io::Result<Vec<u8>> {
        self.codes(INPUT_BITS, kind)
    }

    /// INPUT_STATE: a bit for each key and button that is down (`kind`
    /// EV_KEY) or each switch that is on (EV_SW), as [`Device::bits`]
    /// answers. A paused description is answered with none.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn state(&self, kind: u16) -> io::Result<Vec<u8>> {
        self.codes(INPUT_STATE, kind)
    }

    /// The answer of `request`, which is a bit for each code of `kind`.
    fn codes(&self, request: u32, kind: u16) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0; BITS_MAX];
        let whole = self.file.control(request, u64::from(kind), &mut bytes)? as usize;
        bytes.truncate(whole);
        let used = bytes.iter().rposition(|&byte| byte != 0).map_or(0, |last| last + 1);
        bytes.truncate(used);
        Ok(bytes)
    }

    /// The keys, moving axes, positions and switches the device has.
    ///
    /// # Errors
    ///
    /// The requests' failure.
    pub fn has(&self) -> io::Result<Has> {
        Ok(Has {
            keys: self.bits(EV_KEY)?,
            moves: self.bits(EV_REL)?,
            places: self.bits(EV_ABS)?,
            switches: self.bits(EV_SW)?,
        })
    }

    /// What the device is: a keyboard, a pointer, a tablet.
    ///
    /// # Errors
    ///
    /// The requests' failure.
    pub fn kinds(&self) -> io::Result<Kinds> {
        Ok(self.has()?.kinds())
    }

    /// INPUT_AXIS: the range and the last place of its axis `code` (an
    /// ABS_*).
    ///
    /// # Errors
    ///
    /// The request's failure: EINVAL for an axis the device has not.
    pub fn axis(&self, code: u16) -> io::Result<InputAxis> {
        let mut bytes = [0; InputAxis::SIZE];
        self.file.control(INPUT_AXIS, u64::from(code), &mut bytes)?;
        wire::axis(&bytes).ok_or_else(short)
    }

    /// INPUT_NAME: what the device calls itself.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn name(&self) -> io::Result<String> {
        self.text(INPUT_NAME)
    }

    /// INPUT_PLACE: where it is plugged in, as `usb-1-2`.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn place(&self) -> io::Result<String> {
        self.text(INPUT_PLACE)
    }

    /// INPUT_SERIAL: its serial number, empty if it has none.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn serial(&self) -> io::Result<String> {
        self.text(INPUT_SERIAL)
    }

    /// The answer of `request`, which is text.
    fn text(&self, request: u32) -> io::Result<String> {
        let mut bytes = [0; INPUT_TEXT_MAX as usize];
        let whole = self.file.control(request, 0, &mut bytes)? as usize;
        Ok(wire::text(&bytes[..whole.min(bytes.len())]).to_owned())
    }

    /// INPUT_TIE: ties the description to the holder of radix-display(2)
    /// whose file is in the caller's slot `slot`. From then on it reads
    /// events only while that holder is in front.
    ///
    /// # Errors
    ///
    /// EBUSY for a description that is tied already, and EINVAL if the
    /// slot's file is not the display layer's.
    pub fn tie(&self, slot: u32) -> io::Result<()> {
        self.file.control(INPUT_TIE, u64::from(slot), &mut []).map(|_| ())
    }
}
