// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The requests of a holder's file (radix-display(2)).
//!
//! Each one copies what it reads of the program's buffer into the
//! kernel, has `radix_display::wire` read it, does its work on the layer
//! with the lock held, and writes its answer once the lock has gone. A
//! request that has more to do than fits one hold of the lock (a PUT of
//! many rows, a change that waits for its beat) takes it again for each
//! step, and finds out again each time whether its holder is still in
//! front.

use alloc::sync::Arc;
use alloc::vec::Vec;

use radix_abi::Status;
use radix_abi::display::{
    self as abi, BLOB_BYTES_MAX, BLOB_COLOR_TABLE, BLOB_HDR, BLOB_MODE, Blob, Buffer, DisplayEvent,
    Entry, FORMAT_XRGB8888, Get, KIND_CARD, KIND_PIPE, KIND_PLANE, KIND_SCREEN, Mode, PART_INFO,
    PART_MODES, PART_MONITOR_DATA, PICTURE_KEPT, Put, REASON_NONE, Thing,
};
use radix_abi::errors::{File as FileError, Kernel};
use radix_display::Rect;
use radix_display::layer::{Asked, Made, Unmade};
use radix_display::state::PictureShape;
use radix_display::wire::{self, ChangeRequest, PutRequest};
use radix_mm::Kmalloc;
use radix_mm::bytes::{extend, zeroed};

use super::{
    Layer, Pages, Picture, holder_of, invalid, kmalloc, no_memory, wait_shown, with_layer,
};
use crate::fs::{FileObject, FileRef};

/// The bytes of a change read into the caller's stack: the header and 30
/// entries, which is more than a frame's change has.
const CHANGE_SMALL: usize = abi::Change::SIZE + 30 * Entry::SIZE;
/// The bytes of a PUT's table read into the caller's stack: the header
/// and 16 rectangles.
const PUT_SMALL: usize = Put::SIZE + 16 * abi::PutRect::SIZE;
/// The most bytes of a PUT's rows copied in for one hold of the lock: a
/// row of the widest picture.
const PUT_STEP: usize = 64 * 1024;

const _: () = assert!(PUT_STEP >= abi::SIDE_MAX as usize * 4);
const _: () = assert!(DisplayEvent::SIZE == 40 && Mode::SIZE == 32);

/// Reads bytes of the caller's buffer from an offset in it.
pub type CopyIn<'a> = dyn Fn(u64, &mut [u8]) -> Result<(), Status> + 'a;
/// Writes bytes of the caller's buffer from an offset in it.
pub type CopyOut<'a> = dyn Fn(u64, &[u8]) -> Result<(), Status> + 'a;

/// A request of a holder's file, as the file interface hands it over.
pub struct Call<'a> {
    pub request: u32,
    pub argument: u64,
    /// The bytes of the caller's buffer.
    pub size: u64,
    /// The number of the caller's process.
    pub who: u64,
    /// Whether the description's capability has the write right, which
    /// every request but LIST and GET needs.
    pub may_write: bool,
    /// Reads the caller's buffer from an offset in it.
    pub copy_in: &'a CopyIn<'a>,
    /// Writes the caller's buffer from an offset in it.
    pub copy_out: &'a CopyOut<'a>,
    /// The file in one of the caller's slots, which must have the write
    /// right if asked, and the read right otherwise.
    pub file_in: &'a dyn Fn(u32, bool) -> Result<FileRef, Status>,
    /// Makes a file at least so many bytes long, within what the caller
    /// may write.
    pub grow: &'a dyn Fn(&FileRef, u64) -> Result<(), Status>,
}

impl Call<'_> {
    /// The first `N` bytes of the caller's buffer.
    fn head<const N: usize>(&self) -> Result<[u8; N], Status> {
        let mut bytes = [0; N];
        (self.copy_in)(0, &mut bytes)?;
        Ok(bytes)
    }
}

/// A request of the holder that the description `file` is. Returns the
/// request's result.
///
/// # Errors
///
/// What radix-display(2) says of each request; not_terminal for a
/// request that is none of them.
pub fn control(file: &FileObject, call: &Call<'_>) -> Result<u64, Status> {
    let done = |made: Result<(), Status>| made.map(|()| 0);
    match call.request {
        abi::LIST => list(call),
        abi::GET => get(call),
        abi::MAKE_BUFFER..=abi::ATTENTION_END if !call.may_write => {
            Err(Kernel::MissingRights.status(0))
        }
        abi::MAKE_BUFFER => done(make_buffer(call)),
        abi::MAKE_PICTURE => done(make_picture(file, call)),
        abi::DROP => done(mine(file, |layer, me| {
            let number = u32::try_from(call.argument).map_err(|_| invalid())?;
            layer.holder(me).ok_or_else(gone)?.drop_object(number)
        })),
        abi::MAKE_BLOB => done(make_blob(file, call)),
        abi::CHANGE => done(change(file, call)),
        abi::MOVE_POINTER => done(mine(file, |layer, me| {
            let plane = (call.argument >> 32) as u32;
            let x = i32::from((call.argument >> 16) as u16 as i16);
            let y = i32::from(call.argument as u16 as i16);
            layer.move_pointer(me, plane, x, y)
        })),
        abi::PUT => done(put(file, call)),
        abi::ASK_BEAT => done(mine(file, |layer, me| {
            let pipe = u32::try_from(call.argument).map_err(|_| invalid())?;
            layer.ask_beat(me, pipe)
        })),
        abi::LEND => done(lend(file, call)),
        abi::RETURN => done(mine(file, Layer::give_back)),
        abi::ATTENTION_END => done(mine(file, Layer::attention_end)),
        _ => Err(Kernel::NotTerminal.status(0)),
    }
}

const _: () = assert!(abi::MAKE_BUFFER == abi::GET + 1 && abi::ATTENTION_END == abi::GET + 11);

fn gone() -> Status {
    Kernel::Gone.status(0)
}

/// Runs `with` on the layer with the holder that `file` is.
fn mine<R>(
    file: &FileObject,
    with: impl FnOnce(&mut Layer, u32) -> Result<R, Status>,
) -> Result<R, Status> {
    with_layer(|layer| {
        let me = holder_of(layer, file)?;
        with(layer, me)
    })
}

fn push(out: &mut Vec<u8, Kmalloc>, bytes: &[u8]) -> Result<(), Status> {
    extend(out, bytes).map_err(|_| no_memory())
}

/// LIST: a Thing for every card, screen, pipe and plane, as many as fit
/// the caller's buffer. Returns how many there are.
fn list(call: &Call<'_>) -> Result<u64, Status> {
    let fit = usize::try_from(call.size / Thing::SIZE as u64).unwrap_or(usize::MAX);
    let (total, bytes) = with_layer(|layer| {
        let total = layer.count();
        let mut left = total.min(fit);
        let mut bytes = Vec::new_in(kmalloc());
        bytes.try_reserve_exact(left * Thing::SIZE).map_err(|_| no_memory())?;
        layer.list(&mut |thing| {
            if left > 0 {
                left -= 1;
                // Room was reserved.
                let _ = extend(&mut bytes, &wire::thing_bytes(&thing));
            }
        });
        Ok::<_, Status>((total, bytes))
    })?;
    (call.copy_out)(0, &bytes)?;
    Ok(total as u64)
}

/// GET: the part the caller's Get asks of a thing. Returns the bytes
/// written.
fn get(call: &Call<'_>) -> Result<u64, Status> {
    let asked = wire::read_get(&call.head::<{ Get::SIZE }>()?).ok_or_else(invalid)?;
    let bytes = with_layer(|layer| part(layer, asked.thing, asked.part))?;
    if bytes.len() as u64 > call.size {
        return Err(invalid());
    }
    (call.copy_out)(0, &bytes)?;
    Ok(bytes.len() as u64)
}

/// The bytes of part `part` of thing `thing`.
///
/// # Errors
///
/// not_found for no such thing, and invalid_argument for a part the
/// thing has none of.
fn part(layer: &Layer, thing: u32, part: u32) -> Result<Vec<u8, Kmalloc>, Status> {
    let kind = layer.kind_of(thing).ok_or(FileError::NotFound.status(0))?;
    let mut out = Vec::new_in(kmalloc());
    match (kind, part) {
        (KIND_CARD, PART_INFO) => {
            let info = layer.card_info(thing).ok_or_else(invalid)?;
            push(&mut out, &wire::card_bytes(&info))?;
        }
        (KIND_SCREEN, PART_INFO) => {
            let info = layer.screen_info(thing).ok_or_else(invalid)?;
            push(&mut out, &wire::screen_bytes(&info))?;
        }
        (KIND_SCREEN, PART_MODES) => {
            for mode in layer.screen_modes(thing).ok_or_else(invalid)? {
                push(&mut out, &wire::mode_bytes(mode))?;
            }
        }
        (KIND_SCREEN, PART_MONITOR_DATA) => {
            push(&mut out, layer.monitor_data(thing).ok_or_else(invalid)?)?;
        }
        (KIND_PIPE, PART_INFO) => {
            let info = layer.pipe_info(thing).ok_or_else(invalid)?;
            push(&mut out, &wire::pipe_bytes(&info))?;
        }
        (KIND_PLANE, PART_INFO) => {
            let info = layer.plane_info(thing).ok_or_else(invalid)?;
            push(&mut out, &wire::plane_bytes(&info))?;
        }
        _ => return Err(invalid()),
    }
    Ok(out)
}

/// MAKE_BUFFER: makes the memory file the caller's Buffer names long
/// enough for a picture of its size, and writes back the row length and
/// the bytes.
fn make_buffer(call: &Call<'_>) -> Result<(), Status> {
    let mut asked = wire::read_buffer(&call.head::<{ Buffer::SIZE }>()?).ok_or_else(invalid)?;
    let row = radix_display::row_bytes(asked.format, asked.width).ok_or_else(invalid)?;
    let bytes = radix_display::picture_bytes(asked.format, asked.width, asked.height, row)
        .ok_or_else(invalid)?;
    if with_layer(|layer| layer.kind_of(asked.card)) != Some(KIND_CARD) {
        return Err(invalid());
    }
    let file = (call.file_in)(asked.file, true)?;
    // Only a memory file has pages a picture can hold.
    if crate::fs::shm::pages_of(&file.at.node).is_none() {
        return Err(invalid());
    }
    (call.grow)(&file, bytes)?;
    (asked.row_bytes, asked.bytes) = (row, bytes);
    (call.copy_out)(0, &wire::buffer_bytes(&asked))
}

/// MAKE_PICTURE: makes the picture the caller's Picture describes and
/// writes back its number.
fn make_picture(file: &FileObject, call: &Call<'_>) -> Result<(), Status> {
    let asked = wire::read_picture(&call.head::<{ abi::Picture::SIZE }>()?).ok_or_else(invalid)?;
    if asked.flags & !PICTURE_KEPT != 0 {
        return Err(invalid());
    }
    let kept = asked.flags & PICTURE_KEPT != 0;
    // PUT's rows are FORMAT_XRGB8888, so that is what a kept picture is.
    if kept && asked.format != FORMAT_XRGB8888 {
        return Err(invalid());
    }
    let least = radix_display::row_bytes(asked.format, asked.width).ok_or_else(invalid)?;
    let row = if kept { least } else { asked.row_bytes };
    if !row.is_multiple_of(4) {
        return Err(invalid());
    }
    let bytes = radix_display::picture_bytes(asked.format, asked.width, asked.height, row)
        .ok_or_else(invalid)?;
    let (me, card) =
        mine(file, |layer, me| Ok((me, layer.card(asked.card).cloned().ok_or_else(invalid)?)))?;
    // With no lock held: holding the pages may make many of them, and
    // the card may wait for its device.
    let pages = if kept {
        None
    } else {
        let source = (call.file_in)(asked.file, false)?;
        let shared = crate::fs::shm::pages_of(&source.at.node).ok_or_else(invalid)?;
        Some(Pages::hold(&shared, asked.offset, bytes)?)
    };
    let shape = PictureShape {
        card: asked.card,
        format: asked.format,
        width: asked.width,
        height: asked.height,
    };
    let handle = card.driver.make_picture(&shape, row, pages.as_ref())?;
    // From here the picture lets go of what the card made as it drops.
    let picture = Picture { shape, row_bytes: row, pages, handle, card };
    let picture = Arc::try_new_in(picture, kmalloc()).map_err(|_| no_memory())?;
    let number = with_layer(|layer| layer.holder(me).ok_or_else(gone)?.keep_picture(picture))?;
    let told = (call.copy_out)(wire::PICTURE_NUMBER_AT as u64, &number.to_le_bytes());
    if told.is_err() {
        // A picture whose number the program never learned is no use to
        // it.
        with_layer(|layer| layer.holder(me).map(|holder| holder.drop_object(number)));
    }
    told
}

/// MAKE_BLOB: keeps the value after the caller's Blob and writes back
/// its number. A mode is kept; of the other kinds, which no card of
/// release 1.1 reads, only what they are.
fn make_blob(file: &FileObject, call: &Call<'_>) -> Result<(), Status> {
    let (kind, bytes) = wire::read_blob(&call.head::<{ Blob::SIZE }>()?).ok_or_else(invalid)?;
    let known = matches!(kind, BLOB_MODE | BLOB_COLOR_TABLE | BLOB_HDR);
    let all_there = (Blob::SIZE as u64 + u64::from(bytes)) <= call.size;
    if !known || bytes > BLOB_BYTES_MAX || !all_there {
        return Err(invalid());
    }
    let mode = if kind == BLOB_MODE {
        let mut value = [0; Mode::SIZE];
        if bytes as usize != Mode::SIZE {
            return Err(invalid());
        }
        (call.copy_in)(Blob::SIZE as u64, &mut value)?;
        Some(wire::read_mode(&value).ok_or_else(invalid)?)
    } else {
        None
    };
    let number = mine(file, |layer, me| layer.holder(me).ok_or_else(gone)?.keep_value(kind, mode))?;
    let told = (call.copy_out)(wire::BLOB_NUMBER_AT as u64, &number.to_le_bytes());
    if told.is_err() {
        let _ = mine(file, |layer, me| layer.holder(me).ok_or_else(gone)?.drop_object(number));
    }
    told
}

/// CHANGE: checks the state the caller's Change and its entries ask for
/// and makes it, and writes back which entry failed and why, or that
/// none did.
// Out of line, with a frame of its own for the bytes of a small change.
#[inline(never)]
fn change(file: &FileObject, call: &Call<'_>) -> Result<(), Status> {
    let head = call.head::<{ abi::Change::SIZE }>()?;
    let len = ChangeRequest::bytes_of(&head).ok_or_else(invalid)?;
    // A frame's change is read into the stack: no memory is asked for at
    // each frame.
    let mut small = [0u8; CHANGE_SMALL];
    let mut large;
    let bytes = if len <= CHANGE_SMALL {
        &mut small[..len]
    } else {
        large = zeroed(len, kmalloc()).map_err(|_| no_memory())?;
        &mut large[..]
    };
    (call.copy_in)(0, bytes)?;
    // Read again from the kernel's copy: the program may have changed
    // its count since.
    let request = ChangeRequest::read(bytes).ok_or_else(invalid)?;
    let made = with_layer(|layer| {
        let me = holder_of(layer, file)?;
        if let Some(holder) = layer.holder(me) {
            holder.set_who(call.who);
        }
        layer.change(me, Asked::of(&request), &request)
    });
    let refused = match made {
        Ok(Made::Done) => None,
        Ok(Made::Wait(seq)) => {
            wait_shown(seq)?;
            None
        }
        Err(Unmade::Status(status)) => return Err(status),
        Err(Unmade::Refused(entry, reason)) => Some((entry, reason)),
    };
    let (entry, reason) = refused.unwrap_or((0, REASON_NONE));
    (call.copy_out)(wire::REFUSAL_AT as u64, &wire::refusal(entry, reason))?;
    if refused.is_some() { Err(invalid()) } else { Ok(()) }
}

/// PUT: writes the rows after the caller's Put and its rectangles into a
/// picture its card keeps.
#[inline(never)]
fn put(file: &FileObject, call: &Call<'_>) -> Result<(), Status> {
    let head = call.head::<{ Put::SIZE }>()?;
    let count = PutRequest::count(&head).ok_or_else(invalid)?;
    let len = PutRequest::table_bytes(count).ok_or_else(invalid)?;
    let mut small = [0u8; PUT_SMALL];
    let mut large;
    let table = if len <= PUT_SMALL {
        &mut small[..len]
    } else {
        large = zeroed(len, kmalloc()).map_err(|_| no_memory())?;
        &mut large[..]
    };
    (call.copy_in)(0, table)?;
    let request = PutRequest::read(table, call.size).ok_or_else(invalid)?;
    let (me, picture) = mine(file, |layer, me| {
        let holder = layer.holder(me).ok_or_else(gone)?;
        Ok((me, holder.picture_ref(request.picture).cloned().ok_or_else(invalid)?))
    })?;
    if picture.pages.is_some() {
        // The program draws in such a picture itself.
        return Err(invalid());
    }
    // Every rectangle is checked before a row of any is written.
    let (width, height) = (picture.shape.width, picture.shape.height);
    let mut most = 0;
    for index in 0..request.len() {
        let rect = request.part(index, width, height).ok_or_else(invalid)?.rect;
        most = most.max(rect.width as usize * 4 * rect.height as usize);
    }
    let mut rows = zeroed(most.min(PUT_STEP), kmalloc()).map_err(|_| no_memory())?;
    for index in 0..request.len() {
        let Some(part) = request.part(index, width, height) else { continue };
        let row = part.rect.width as usize * 4;
        let (mut done, all) = (0, part.rect.height);
        while done < all {
            // Whole rows, as many as fit a step: a row always does.
            let step = (all - done).min((PUT_STEP / row) as u32);
            let bytes = &mut rows[..step as usize * row];
            (call.copy_in)(part.offset + u64::from(done) * row as u64, bytes)?;
            let rect = Rect { y: part.rect.y + done as i32, height: step, ..part.rect };
            with_layer(|layer| layer.put(me, &picture, rect, bytes))?;
            done += step;
        }
    }
    Ok(())
}

/// LEND: lends the front to the holder whose description is in the slot
/// the caller names.
fn lend(file: &FileObject, call: &Call<'_>) -> Result<(), Status> {
    let slot = u32::try_from(call.argument).map_err(|_| invalid())?;
    let to = (call.file_in)(slot, false)?;
    if !to.is_display() || to.address() == file.address() {
        return Err(invalid());
    }
    with_layer(|layer| {
        let me = holder_of(layer, file)?;
        let borrower = holder_of(layer, &to)?;
        layer.lend(me, borrower)
    })
}
