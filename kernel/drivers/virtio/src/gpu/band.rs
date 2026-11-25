// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How rows reach a picture the host keeps.
//!
//! The host copies a rectangle of a resource from the resource's
//! backing. A picture a program draws in is its own backing. One the
//! host keeps for whoever writes rows into it (a text console, the
//! kernel's log) needs a backing only while the rows travel, and has one
//! of two kinds:
//!
//! - the *band* ([`Band`]): one store of a few rows that is the backing
//!   of every such resource of the card. The rectangles written are
//!   placed in it side by side and one under another ([`Sheet`]), and
//!   it is used again once the host has copied them. A screen of text
//!   then costs the band, whatever the screen's size;
//! - a store of its own, as large as the resource ([`put_whole`]), for
//!   a host that refuses the band's transfers.
//!
//! Neither waits for the host: the commands are queued, the host is
//! told, and the caller goes on. The band waits only when it is full of
//! rows the host has not copied yet.
//!
//! The rows are asked for one at a time ([`Rows`]), so that whoever
//! draws them, as the kernel's log does its text, needs room for one
//! row and no more. An area of one colour is cheaper still
//! ([`Band::fill`]): a strip of it goes in the band once and every part
//! of the area is copied from that strip.

use radix_hw::Dma;

use super::{Gpu, Rect};
use crate::Error;

/// Where rectangles of rows go in a store too short for the resource
/// they are sent to.
///
/// The store holds `rows` rows of the resource's width. A transfer reads
/// row `n` of its rectangle a whole resource row after row `n - 1`, so a
/// rectangle fits anywhere in the store that has its width and height
/// free, whatever its place in the resource, and several fit side by
/// side and one under another until the store is full: shelves, each as
/// tall as its tallest rectangle.
#[derive(Clone, Copy, Debug)]
pub struct Sheet {
    width: u32,
    rows: u32,
    /// Where the next rectangle of the shelf goes.
    x: u32,
    /// The shelf's top row, and its height.
    top: u32,
    tall: u32,
}

impl Sheet {
    /// The store of `bytes` bytes for a resource `width` pixels wide.
    pub fn new(width: u32, bytes: usize) -> Sheet {
        let row = (width as usize).max(1) * 4;
        Sheet { width, rows: u32::try_from(bytes / row).unwrap_or(u32::MAX), x: 0, top: 0, tall: 0 }
    }

    /// The width of the resource it is for.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// How many rows the store holds: a taller rectangle goes in parts.
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// Forgets what was placed.
    pub fn clear(&mut self) {
        (self.x, self.top, self.tall) = (0, 0, 0);
    }

    /// Finds room for a rectangle of `width` by `height` and returns
    /// where its first byte goes in the store, which is the transfer's
    /// `offset`. `None` if it does not fit beside or under what is
    /// placed: the store is sent and cleared first. One wider than the
    /// resource or taller than the store never fits.
    pub fn place(&mut self, width: u32, height: u32) -> Option<u64> {
        if width == 0 || height == 0 || width > self.width || height > self.rows {
            return None;
        }
        let (x, top) = if self.x + width > self.width {
            // A new shelf under this one.
            (0, self.top + self.tall)
        } else {
            (self.x, self.top)
        };
        if top + height > self.rows {
            return None;
        }
        self.tall = if top == self.top { self.tall.max(height) } else { height };
        (self.x, self.top) = (x + width, top);
        Some((u64::from(top) * u64::from(self.width) + u64::from(x)) * 4)
    }
}

/// A resource rows are sent to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// The resource.
    pub resource: u32,
    /// Its width in pixels.
    pub width: u32,
    /// Whether a screen shows it, and is to show the rows at once.
    pub shown: bool,
}

/// The rows of a rectangle, from its top down: each is the rectangle's
/// width in pixels of four bytes. A row of any other length ends the
/// write with [`Error::BadRequest`].
pub trait Rows {
    /// Row `index` of the rectangle.
    fn row(&mut self, index: u32) -> &[u8];
}

/// Rows that lie one after another in memory.
#[derive(Clone, Copy, Debug)]
pub struct Packed<'a> {
    bytes: &'a [u8],
    row: usize,
}

impl<'a> Packed<'a> {
    /// The rows of `rect` in `bytes`, or `None` if `bytes` is not the
    /// rectangle's pixels and nothing else.
    pub fn new(rect: Rect, bytes: &'a [u8]) -> Option<Packed<'a>> {
        let all = u64::from(rect.width) * 4 * u64::from(rect.height);
        let whole = rect.width > 0 && rect.height > 0 && bytes.len() as u64 == all;
        whole.then_some(Packed { bytes, row: rect.width as usize * 4 })
    }
}

impl Rows for Packed<'_> {
    fn row(&mut self, index: u32) -> &[u8] {
        let from = index as usize * self.row;
        self.bytes.get(from..from + self.row).unwrap_or(&[])
    }
}

/// One row, for every row of a rectangle: an area of one colour.
#[derive(Clone, Copy, Debug)]
pub struct Same<'a>(pub &'a [u8]);

impl Rows for Same<'_> {
    fn row(&mut self, _index: u32) -> &[u8] {
        self.0
    }
}

/// The text band of a card.
pub struct Band {
    memory: Dma,
    sheet: Sheet,
}

impl Band {
    /// The band in `memory`, which stays the device's until the card is
    /// reset.
    pub fn new(memory: Dma) -> Band {
        Band { memory, sheet: Sheet::new(0, 0) }
    }

    /// Its address and its length: the backing of every resource it
    /// carries rows to.
    pub fn backing(&self) -> (u64, u32) {
        (self.memory.phys(), self.memory.len() as u32)
    }

    /// Sends `rows`, the pixels of `rect`, to the resource `to`, whose
    /// backing the band is. It waits, through `wait`, only for the host
    /// to copy what fills the band.
    ///
    /// # Errors
    ///
    /// [`Error::BadRequest`] for an empty rectangle, a row that is not
    /// the rectangle's width or a resource too wide for the band to hold
    /// a row of, and otherwise as [`Gpu::finish`].
    pub fn put(
        &mut self,
        gpu: &mut Gpu,
        to: Target,
        rect: Rect,
        rows: &mut dyn Rows,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if rect.width == 0 || rect.height == 0 {
            return Err(Error::BadRequest);
        }
        gpu.loosely(|gpu| self.place_all(gpu, to, rect, rows, wait))
    }

    fn place_all(
        &mut self,
        gpu: &mut Gpu,
        to: Target,
        rect: Rect,
        rows: &mut dyn Rows,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.open(gpu, to.width, wait)?;
        let (row, stride) = (rect.width as usize * 4, to.width as usize * 4);
        let mut done = 0;
        while done < rect.height {
            let tall = (rect.height - done).min(self.sheet.rows());
            let at = self.room(gpu, rect.width, tall, wait)?;
            for line in 0..tall {
                let pixels = rows.row(done + line);
                if pixels.len() != row {
                    return Err(Error::BadRequest);
                }
                self.memory.write(at as usize + line as usize * stride, pixels);
            }
            gpu.transfer(to.resource, Rect { y: rect.y + done, height: tall, ..rect }, at)?;
            done += tall;
        }
        self.show(gpu, to, rect, wait)
    }

    /// Paints `rect` of the resource `to`, whose backing the band is,
    /// with `row` on every row of it. As many rows as the band holds go
    /// in it once, and each part of the rectangle is copied from them:
    /// it waits, through `wait`, only when no more commands can be
    /// queued.
    ///
    /// # Errors
    ///
    /// As [`Band::put`].
    pub fn fill(
        &mut self,
        gpu: &mut Gpu,
        to: Target,
        rect: Rect,
        row: &[u8],
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if rect.width == 0 || rect.height == 0 || row.len() != rect.width as usize * 4 {
            return Err(Error::BadRequest);
        }
        gpu.loosely(|gpu| self.fill_all(gpu, to, rect, row, wait))
    }

    fn fill_all(
        &mut self,
        gpu: &mut Gpu,
        to: Target,
        rect: Rect,
        row: &[u8],
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.open(gpu, to.width, wait)?;
        let tall = rect.height.min(self.sheet.rows());
        let at = self.room(gpu, rect.width, tall, wait)?;
        for line in 0..tall as usize {
            self.memory.write(at as usize + line * to.width as usize * 4, row);
        }
        let mut done = 0;
        while done < rect.height {
            let part = (rect.height - done).min(tall);
            if !gpu.has_room() {
                // The strip stays where it is: only the commands go.
                gpu.finish(wait)?;
            }
            gpu.transfer(to.resource, Rect { y: rect.y + done, height: part, ..rect }, at)?;
            done += part;
        }
        self.show(gpu, to, rect, wait)
    }

    /// Makes the band one for resources `width` pixels wide, once
    /// nothing reads rows of another length from it, and clears it if
    /// the host has copied all that was in it.
    fn open(
        &mut self,
        gpu: &mut Gpu,
        width: u32,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if self.sheet.width() != width {
            gpu.finish(wait)?;
            self.sheet = Sheet::new(width, self.memory.len());
        } else if gpu.collect() == 0 {
            self.sheet.clear();
        }
        Ok(())
    }

    /// Finds room for a rectangle of `width` by `tall` and for the
    /// command that sends it, after the host has copied what is in the
    /// way, and returns where the rectangle's first byte goes.
    fn room(
        &mut self,
        gpu: &mut Gpu,
        width: u32,
        tall: u32,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<u64, Error> {
        if !gpu.has_room() {
            self.empty(gpu, wait)?;
        }
        if let Some(at) = self.sheet.place(width, tall) {
            return Ok(at);
        }
        self.empty(gpu, wait)?;
        self.sheet.place(width, tall).ok_or(Error::BadRequest)
    }

    /// Has the screens that show `to` show `rect` of it.
    fn show(
        &mut self,
        gpu: &mut Gpu,
        to: Target,
        rect: Rect,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if !to.shown {
            return Ok(());
        }
        if !gpu.has_room() {
            self.empty(gpu, wait)?;
        }
        gpu.flush(to.resource, rect)
    }

    /// Waits for the host to copy what is in the band, and clears it.
    fn empty(&mut self, gpu: &mut Gpu, wait: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        gpu.finish(wait)?;
        self.sheet.clear();
        Ok(())
    }
}

/// Sends `rows`, the pixels of `rect`, to the resource `to`, whose
/// backing is `store`, as large as the resource: the rows go at their
/// own place in it. It waits, through `wait`, only when no more commands
/// can be queued.
///
/// # Errors
///
/// [`Error::BadRequest`] for an empty rectangle, one outside the store
/// or a row that is not the rectangle's width, and otherwise as
/// [`Gpu::finish`].
pub fn put_whole(
    gpu: &mut Gpu,
    store: &Dma,
    to: Target,
    rect: Rect,
    rows: &mut dyn Rows,
    wait: &mut dyn FnMut() -> bool,
) -> Result<(), Error> {
    let (row, stride) = (rect.width as usize * 4, to.width as usize * 4);
    let at = rect.y as usize * stride + rect.x as usize * 4;
    let inside = u64::from(rect.x) + u64::from(rect.width) <= u64::from(to.width)
        && (at + (rect.height as usize).saturating_sub(1) * stride).saturating_add(row)
            <= store.len();
    if rect.width == 0 || rect.height == 0 || !inside {
        return Err(Error::BadRequest);
    }
    for line in 0..rect.height {
        let pixels = rows.row(line);
        if pixels.len() != row {
            return Err(Error::BadRequest);
        }
        store.write(at + line as usize * stride, pixels);
    }
    gpu.loosely(|gpu| {
        if !gpu.has_room() {
            gpu.finish(wait)?;
        }
        gpu.transfer(to.resource, rect, at as u64)?;
        if to.shown {
            if !gpu.has_room() {
                gpu.finish(wait)?;
            }
            gpu.flush(to.resource, rect)?;
        }
        Ok(())
    })
}
