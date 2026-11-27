// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The requests of the input files (radix-input(2)).
//!
//! Each one works out its answer with the hub's lock held, in the
//! kernel's memory, and writes it to the program's buffer once the lock
//! has gone. A request that fills the buffer with as much as fits writes
//! that much and returns the length of the whole answer.

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::input::{self as abi, INPUT_TEXT_MAX, InputAxis, InputInfo, KEY_CODES};
use radix_input::device::{Device, text};
use radix_input::event::Record;
use radix_input::hub::{DEVICES_MAX, Source};

use super::{keyboard, refused, source_of, with_reader};
use crate::fs::{FileObject, FileRef};
use crate::sched;

/// The bytes of the longest answer about a device: a bit for each key.
const ANSWER_MAX: usize = KEY_CODES as usize / 8;

const _: () = assert!(ANSWER_MAX >= INPUT_TEXT_MAX as usize && ANSWER_MAX >= InputAxis::SIZE);

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
        abi::INPUT_LIST => list(file, call),
        abi::INPUT_INFO..=abi::INPUT_SERIAL => about(file, call),
        _ => Err(Kernel::NotTerminal.status(0)),
    }
}

const _: () = assert!(abi::INPUT_TIE > abi::INPUT_INFO && abi::INPUT_TIE < abi::INPUT_SERIAL);

/// INPUT_TIE: ties the reader to the holder that the display file in
/// the slot `argument` names is.
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

/// INPUT_LIST: the number of each device, as many as fit. Returns how
/// many there are.
fn list(file: &FileObject, call: &Call<'_>) -> Result<u64, Status> {
    if source_of(file) != Some(Source::Control) {
        return Err(Kernel::NotTerminal.status(0));
    }
    let mut numbers = [0u32; DEVICES_MAX];
    let count = with_reader(file, |hub, _| Ok(hub.list(&mut numbers)))?.map_err(refused)?;
    let fit = usize::try_from(call.size / 4).unwrap_or(usize::MAX).min(count);
    let mut bytes = [0u8; DEVICES_MAX * 4];
    for (out, number) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(&numbers[..fit]) {
        *out = number.to_le_bytes();
    }
    (call.copy_out)(&bytes[..fit * 4])?;
    Ok(count as u64)
}

/// The requests that ask a device's file about its device: fills the
/// caller's buffer with the answer, as much as fits, and returns the
/// length of the whole.
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
