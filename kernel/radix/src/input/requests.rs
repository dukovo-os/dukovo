// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The requests of the input files (radix-input(2)).
//!
//! Each one works out its answer with the hub's lock held, in the
//! kernel's memory, and writes it to the program's buffer once the lock
//! has gone. A request that fills the buffer with as much as fits writes
//! that much and returns the length of the whole answer.

use alloc::vec::Vec;

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::input::{self as abi, INPUT_TEXT_MAX, InputAxis, InputInfo, KEY_CODES};
use radix_input::device::{Device, text};
use radix_input::event::Record;
use radix_input::hub::{DEVICES_MAX, Source};
use radix_mm::{Flags, HEAP, Kmalloc};

use super::{keyboard, refused, source_of, with_reader};
use crate::fs::{FileObject, FileRef};
use crate::sched;

/// The bytes of the longest answer about a device: a bit for each key.
const ANSWER_MAX: usize = KEY_CODES as usize / 8;

const _: () = assert!(ANSWER_MAX >= INPUT_TEXT_MAX as usize && ANSWER_MAX >= InputAxis::SIZE);

/// Every lock light there is.
const LIGHTS: u32 = abi::LED_SCROLL_LOCK | abi::LED_NUM_LOCK | abi::LED_CAPS_LOCK;

/// A request of an input file, as the file interface hands it over.
pub struct Call<'a> {
    pub request: u32,
    pub argument: u64,
    /// The bytes of the caller's buffer.
    pub size: u64,
    /// Whether the description's capability has the write right, which
    /// SET_LEDS needs.
    pub may_write: bool,
    /// Writes the start of the caller's buffer.
    pub copy_out: &'a dyn Fn(&[u8]) -> Result<(), Status>,
    /// The file in one of the caller's slots, whatever its rights.
    pub file_in: &'a dyn Fn(u32) -> Result<FileRef, Status>,
}

fn invalid() -> Status {
    Kernel::InvalidArgument.status(0)
}

/// A request of the input file that the description `file` is of.
/// Returns the request's result.
///
/// # Errors
///
/// What radix-input(2) says of each request; not_terminal for a request
/// the file has not.
pub fn control(file: &FileObject, call: &Call<'_>) -> Result<u64, Status> {
    match call.request {
        abi::SET_LEDS if source_of(file) == Some(Source::Keyboard) => {
            if !call.may_write {
                return Err(Kernel::MissingRights.status(0));
            }
            keyboard::set_lights(call.argument as u32);
            Ok(0)
        }
        abi::INPUT_TIE => tie(file, call),
        abi::INPUT_LIGHTS => lights(file, call),
        abi::INPUT_LIST => list(file, call),
        abi::INPUT_INFO..=abi::INPUT_SERIAL => about(file, call),
        _ => Err(Kernel::NotTerminal.status(0)),
    }
}

const _: () = assert!(abi::INPUT_TIE > abi::INPUT_INFO && abi::INPUT_TIE < abi::INPUT_SERIAL);

/// INPUT_TIE: ties the reader to the holder that the display file in
/// the slot `argument` names is.
#[inline(never)]
fn tie(file: &FileObject, call: &Call<'_>) -> Result<u64, Status> {
    let slot = u32::try_from(call.argument).map_err(|_| invalid())?;
    let display = (call.file_in)(slot)?;
    // With the display layer's lock held: the holder does not end, nor
    // the front pass, between its being found and the tie.
    crate::display::with_holder(&display, |holder| {
        let now = sched::now_ns();
        with_reader(file, |hub, key| hub.tie(key, holder, now))?.map_err(refused)
    })??;
    Ok(0)
}

/// INPUT_LIGHTS: a reader that is heard, of a device that has lock
/// lights, sets the lights of every keyboard, as SET_LEDS does.
#[inline(never)]
fn lights(file: &FileObject, call: &Call<'_>) -> Result<u64, Status> {
    let lights = u32::try_from(call.argument).ok().filter(|lights| lights & !LIGHTS == 0);
    let lights = lights.ok_or_else(invalid)?;
    let heard = with_reader(file, |hub, key| {
        let (device, heard) = hub.asked(key)?;
        Ok((!device.about.lights.is_empty()).then_some(heard))
    })?
    .map_err(refused)?
    .ok_or_else(invalid)?;
    if !heard {
        return Err(Kernel::Busy.status(0));
    }
    keyboard::set_lights(lights);
    Ok(0)
}

/// INPUT_LIST: the number of each device, as many as fit. Returns how
/// many there are.
#[inline(never)]
fn list(file: &FileObject, call: &Call<'_>) -> Result<u64, Status> {
    if source_of(file) != Some(Source::Control) {
        return Err(Kernel::NotTerminal.status(0));
    }
    // On the heap: a number for every device there may be is more than
    // a frame of the kernel's stack is for (ADR 0058).
    let room = usize::try_from(call.size).unwrap_or(usize::MAX).min(DEVICES_MAX * 4) & !3;
    let mut bytes: Vec<u8, Kmalloc> = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    bytes.try_reserve_exact(room).map_err(|_| Kernel::NoMemory.status(0))?;
    while bytes.len() < room {
        // Room was reserved above.
        let _ = bytes.push_within_capacity(0);
    }
    let count = with_reader(file, |hub, _| Ok(hub.list(&mut bytes)))?.map_err(refused)?;
    (call.copy_out)(&bytes[..room.min(count * 4)])?;
    Ok(count as u64)
}

/// The requests that ask a device's file about its device: fills the
/// caller's buffer with the answer, as much as fits, and returns the
/// length of the whole.
#[inline(never)]
fn about(file: &FileObject, call: &Call<'_>) -> Result<u64, Status> {
    let mut answer = [0u8; ANSWER_MAX];
    let whole = with_reader(file, |hub, key| {
        let (device, heard) = hub.asked(key)?;
        Ok(answer_of(device, heard, call, &mut answer))
    })?
    .map_err(refused)?
    .ok_or_else(invalid)?;
    let size = usize::try_from(call.size).unwrap_or(usize::MAX);
    // A structure is written whole or not at all.
    let whole_only = matches!(call.request, abi::INPUT_INFO | abi::INPUT_AXIS);
    if whole_only && size < whole {
        return Err(invalid());
    }
    (call.copy_out)(&answer[..whole.min(size)])?;
    Ok(whole as u64)
}

/// Writes the answer of `call` about `device` into `out`, for a reader
/// that is `heard` or is not, and returns its length; `None` for an
/// argument the request does not take.
fn answer_of(
    device: &Device,
    heard: bool,
    call: &Call<'_>,
    out: &mut [u8; ANSWER_MAX],
) -> Option<usize> {
    let code = u16::try_from(call.argument).ok();
    let mut put = |field: &[u8]| {
        let field = text(field).as_bytes();
        out[..field.len()].copy_from_slice(field);
        field.len()
    };
    Some(match call.request {
        abi::INPUT_NAME => put(&device.about.name),
        abi::INPUT_PLACE => put(&device.about.place),
        abi::INPUT_SERIAL => put(&device.about.serial),
        abi::INPUT_INFO => {
            device.info().encode(&mut out[..InputInfo::SIZE]);
            InputInfo::SIZE
        }
        abi::INPUT_AXIS => {
            device.axis(code?, heard)?.encode(&mut out[..InputAxis::SIZE]);
            InputAxis::SIZE
        }
        abi::INPUT_BITS => device.about.bits(code?, out)?,
        abi::INPUT_STATE => device.state(code?, heard, out)?,
        _ => return None,
    })
}
