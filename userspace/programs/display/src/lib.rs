// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The display layer for programs (release 1.1, ADR 0700): what a program
//! needs to draw on the machine's screens through
//! _/devices/display/control_ (radix-display(2)).
//!
//! - [`wire`] lays out the interface's records as a request's `buffer`
//!   holds them, and reads what the kernel writes back.
//! - [`Holder`] sends the twelve requests, each with its record, over
//!   whatever sends a `control` request to the program's description of
//!   the file ([`Control`]).
//!
//! Nothing here opens a file or makes a system call of its own, so its
//! tests run on the build machine, where they check each record against
//! the kernel's own reader or writer of it. A program implements
//! [`Control`] with radix-file(2)'s `control`, reads its events from the
//! file itself, and gives the bytes to [`wire::events`].

pub mod wire;

#[cfg(test)]
mod tests;

use std::io;

pub use radix_abi::display as abi;
use radix_abi::display::{
    ASK_BEAT, Buffer, CHANGE, CardInfo, DROP, FORMAT_XRGB8888, GET, Get, LEND, LIST, MAKE_BLOB,
    MAKE_BUFFER, MAKE_PICTURE, MOVE_POINTER, Mode, PART_INFO, PART_MODES, PART_MONITOR_DATA,
    PICTURE_KEPT, PUT, Picture, PipeInfo, PlaneInfo, RETURN, ScreenInfo, Thing,
};

use wire::{ChangeBytes, PutBytes};

/// How a holder's requests reach its file.
pub trait Control {
    /// Sends the file `request` with `argument`, and `buffer` for the
    /// kernel to read and write back, as radix-file(2)'s `control` does.
    /// Returns the request's result.
    ///
    /// # Errors
    ///
    /// The request's failure, with the error number the C library gives
    /// it: EBUSY for busy, EINVAL for invalid_argument.
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64>;
}

/// Why a change was not made.
#[derive(Debug)]
pub enum Unmade {
    /// The check refused an entry: which one, from 0, and a REASON_*.
    /// The entry is the number of entries when the fault is in the
    /// state the change started from.
    Refused {
        /// The entry.
        entry: u32,
        /// Why.
        reason: u32,
    },
    /// The request failed: EBUSY for a holder that is not in front, or
    /// whose earlier change of a pipe still waits.
    Failed(io::Error),
}

impl core::fmt::Display for Unmade {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Unmade::Refused { entry, reason } => {
                write!(f, "entry {entry} was refused for reason {reason}")
            }
            Unmade::Failed(error) => error.fmt(f),
        }
    }
}

/// The reply of the kernel was not the record it should be.
fn short() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "the display layer's answer was cut short")
}

/// A holder: a description of _/devices/display/control_, and the
/// requests on it.
#[derive(Debug)]
pub struct Holder<C> {
    file: C,
}

impl<C: Control> Holder<C> {
    /// The holder whose description `file` reaches.
    pub fn new(file: C) -> Holder<C> {
        Holder { file }
    }

    /// What reaches its description.
    pub fn file(&self) -> &C {
        &self.file
    }

    /// LIST: every card, screen, pipe and plane.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn things(&self) -> io::Result<Vec<Thing>> {
        let mut room = 32;
        loop {
            let mut bytes = vec![0; room * Thing::SIZE];
            let count = self.file.control(LIST, 0, &mut bytes)? as usize;
            if count <= room {
                return Ok(wire::things(&bytes[..count * Thing::SIZE]).collect());
            }
            // More than fit, or some came since: again, with room.
            room = count;
        }
    }

    /// GET: `part` of `thing` into `bytes`. Returns the bytes written.
    fn get(&self, thing: u32, part: u32, bytes: &mut [u8]) -> io::Result<usize> {
        let asked = wire::get(thing, part);
        let head = bytes.get_mut(..Get::SIZE).ok_or_else(short)?;
        head.copy_from_slice(&asked);
        let written = self.file.control(GET, 0, bytes)? as usize;
        if written > bytes.len() {
            return Err(short());
        }
        Ok(written)
    }

    /// What card `id` is.
    ///
    /// # Errors
    ///
    /// ENOENT if there is no such thing, and EINVAL if it is no card.
    pub fn card(&self, id: u32) -> io::Result<CardInfo> {
        let mut bytes = [0; CardInfo::SIZE];
        let written = self.get(id, PART_INFO, &mut bytes)?;
        wire::card(&bytes[..written]).ok_or_else(short)
    }

    /// What screen `id` is.
    ///
    /// # Errors
    ///
    /// ENOENT if there is no such thing, and EINVAL if it is no screen.
    pub fn screen(&self, id: u32) -> io::Result<ScreenInfo> {
        let mut bytes = [0; ScreenInfo::SIZE];
        let written = self.get(id, PART_INFO, &mut bytes)?;
        wire::screen(&bytes[..written]).ok_or_else(short)
    }

    /// The modes of `screen`, the preferred one first.
    ///
    /// # Errors
    ///
    /// The request's failure: EINVAL if the screen has more modes now
    /// than `screen` says, which a new listing puts right.
    pub fn modes(&self, screen: &ScreenInfo) -> io::Result<Vec<Mode>> {
        let mut bytes = vec![0; (screen.modes as usize * Mode::SIZE).max(Get::SIZE)];
        let written = self.get(screen.id, PART_MODES, &mut bytes)?;
        Ok(wire::modes(&bytes[..written]).collect())
    }

    /// The bytes the monitor of `screen` describes itself with.
    ///
    /// # Errors
    ///
    /// The request's failure, as [`Holder::modes`]'s.
    pub fn monitor_data(&self, screen: &ScreenInfo) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0; (screen.monitor_data_bytes as usize).max(Get::SIZE)];
        let written = self.get(screen.id, PART_MONITOR_DATA, &mut bytes)?;
        bytes.truncate(written);
        Ok(bytes)
    }

    /// What pipe `id` is, and its beat.
    ///
    /// # Errors
    ///
    /// ENOENT if there is no such thing, and EINVAL if it is no pipe.
    pub fn pipe(&self, id: u32) -> io::Result<PipeInfo> {
        let mut bytes = [0; PipeInfo::SIZE];
        let written = self.get(id, PART_INFO, &mut bytes)?;
        wire::pipe(&bytes[..written]).ok_or_else(short)
    }

    /// What plane `id` is.
    ///
    /// # Errors
    ///
    /// ENOENT if there is no such thing, and EINVAL if it is no plane.
    pub fn plane(&self, id: u32) -> io::Result<PlaneInfo> {
        let mut bytes = [0; PlaneInfo::SIZE];
        let written = self.get(id, PART_INFO, &mut bytes)?;
        wire::plane(&bytes[..written]).ok_or_else(short)
    }

    /// MAKE_BUFFER: makes the memory file in `buffer.file` long enough
    /// for the picture `buffer` describes. Returns the bytes from one of
    /// its rows to the next, and the bytes of all of it.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn make_buffer(&self, buffer: &Buffer) -> io::Result<(u32, u64)> {
        let mut bytes = wire::buffer(buffer);
        self.file.control(MAKE_BUFFER, 0, &mut bytes)?;
        wire::buffer_made(&bytes).ok_or_else(short)
    }

    /// MAKE_PICTURE: makes the picture `picture` describes, and returns
    /// its number.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn make_picture(&self, picture: &Picture) -> io::Result<u32> {
        let mut bytes = wire::picture(picture);
        self.file.control(MAKE_PICTURE, 0, &mut bytes)?;
        wire::picture_made(&bytes).ok_or_else(short)
    }

    /// Makes a picture of `width` by `height` that `card` keeps, whose
    /// rows PUT sends, and returns its number.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn keep_picture(&self, card: u32, width: u32, height: u32) -> io::Result<u32> {
        self.make_picture(&Picture {
            card,
            flags: PICTURE_KEPT,
            format: FORMAT_XRGB8888,
            width,
            height,
            ..Picture::default()
        })
    }

    /// MAKE_BLOB: keeps `mode` as a value, for PIPE_MODE, and returns
    /// its number.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn make_mode(&self, mode: &Mode) -> io::Result<u32> {
        let mut bytes = wire::mode_blob(mode);
        self.file.control(MAKE_BLOB, 0, &mut bytes)?;
        wire::blob_made(&bytes).ok_or_else(short)
    }

    /// DROP: lets go of picture or value `number`.
    ///
    /// # Errors
    ///
    /// EBUSY while the holder's state shows the picture or uses the
    /// value.
    pub fn drop_object(&self, number: u32) -> io::Result<()> {
        self.file.control(DROP, u64::from(number), &mut []).map(|_| ())
    }

    /// CHANGE: checks the state `change` asks for and makes it.
    ///
    /// # Errors
    ///
    /// The entry the check refused and why, or the request's failure.
    pub fn change(&self, change: &mut ChangeBytes) -> Result<(), Unmade> {
        match self.file.control(CHANGE, 0, change.bytes_mut()) {
            Ok(_) => Ok(()),
            Err(error) => Err(match change.refusal() {
                Some((entry, reason)) => Unmade::Refused { entry, reason },
                None => Unmade::Failed(error),
            }),
        }
    }

    /// MOVE_POINTER: puts the hot spot of pointer plane `plane` at
    /// (`x`, `y`) of its pipe's frame, at once.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn move_pointer(&self, plane: u32, x: i16, y: i16) -> io::Result<()> {
        self.file.control(MOVE_POINTER, wire::pointer_place(plane, x, y), &mut []).map(|_| ())
    }

    /// PUT: writes the rows of `put` into `picture`, which its card
    /// keeps.
    ///
    /// # Errors
    ///
    /// EBUSY for a holder that is not in front.
    pub fn put(&self, picture: u32, put: &mut PutBytes) -> io::Result<()> {
        self.file.control(PUT, 0, put.bytes_for(picture)).map(|_| ())
    }

    /// ASK_BEAT: asks for an EVENT_BEAT at the next beat of `pipe`.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn ask_beat(&self, pipe: u32) -> io::Result<()> {
        self.file.control(ASK_BEAT, u64::from(pipe), &mut []).map(|_| ())
    }

    /// LEND: lends the front to the holder whose description is in the
    /// caller's slot `slot`.
    ///
    /// # Errors
    ///
    /// EBUSY unless the caller is in front.
    pub fn lend(&self, slot: u32) -> io::Result<()> {
        self.file.control(LEND, u64::from(slot), &mut []).map(|_| ())
    }

    /// RETURN: gives the front back to the holder that lent it.
    ///
    /// # Errors
    ///
    /// The request's failure.
    pub fn give_back(&self) -> io::Result<()> {
        self.file.control(RETURN, 0, &mut []).map(|_| ())
    }
}
