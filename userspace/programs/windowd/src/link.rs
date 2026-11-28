// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How windowd reaches the display layer (radix-display(2)): its
//! requests, and pixels the renderer draws a picture's in.
//!
//! [`Card`] is the holder consoled lent. A picture's pixels are the
//! pages of a memory file, which the renderer maps and draws in, and
//! which the card reads at a beat or at any moment
//! (`CARD_SHARED_PAGES`): no frame is copied on its way to a screen.
//!
//! The tests give [`crate::screens::Screens`] a link of their own, which
//! keeps each request and gives pixels of pixman's.

use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use display::abi::{Buffer, Picture};
use display::{Control, Holder};
use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufFlags};
use smithay::backend::allocator::{Fourcc, Modifier};
use sysapi::kernel;
use sysapi::memory::MemoryFile;

/// A holder's file, and the pixels of the pictures made on it.
pub trait Link: Control + Sized {
    /// What the renderer draws a picture's pixels through.
    type Pixels;

    /// Makes a picture of `format` and `size` (width, height) on `card`
    /// that the program draws itself. Returns its number and its
    /// pixels.
    ///
    /// # Errors
    ///
    /// What the layer's requests fail with, and ENOSPC or ENOMEM when
    /// the system has no room for the pixels.
    fn pixels(
        holder: &Holder<Self>,
        card: u32,
        format: u32,
        size: (u32, u32),
    ) -> io::Result<(u32, Self::Pixels)>;
}

/// The description of _/devices/display/control_ that consoled lent.
#[derive(Debug)]
pub struct Card(File);

impl Card {
    /// The holder `lent` is, whose events are read without waiting.
    ///
    /// # Errors
    ///
    /// What fcntl(2) fails with.
    pub fn new(lent: OwnedFd) -> io::Result<Card> {
        sysapi::fd::set_nonblocking(&lent, true)?;
        Ok(Card(File::from(lent)))
    }

    /// Reads the events that wait into `into`. Returns their bytes,
    /// which is 0 when none waits.
    ///
    /// # Errors
    ///
    /// What read(2) fails with but EAGAIN.
    pub fn events(&self, into: &mut [u8]) -> io::Result<usize> {
        match (&self.0).read(into) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(0),
            read => read,
        }
    }
}

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl Control for Card {
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
        kernel::control(&self.0, request, argument, buffer).map_err(kernel::io_error)
    }
}

impl Link for Card {
    type Pixels = Dmabuf;

    fn pixels(
        holder: &Holder<Card>,
        card: u32,
        format: u32,
        (width, height): (u32, u32),
    ) -> io::Result<(u32, Dmabuf)> {
        let file = MemoryFile::new()?;
        let slot = file.slot();
        let asked = Buffer { file: slot, card, format, width, height, ..Buffer::default() };
        let (row_bytes, _) = holder.make_buffer(&asked)?;
        let described =
            Picture { file: slot, card, format, width, height, row_bytes, ..Picture::default() };
        let picture = holder.make_picture(&described)?;
        let unknown = || io::Error::new(io::ErrorKind::Unsupported, "a format pixman has not");
        let fourcc = Fourcc::try_from(format).map_err(|_| unknown())?;
        let size = (width as i32, height as i32);
        let mut pixels = Dmabuf::builder(size, fourcc, Modifier::Linear, DmabufFlags::empty());
        // The renderer maps the file's pages, and nothing else of the
        // program reaches them.
        pixels.add_plane(file.into_fd()?, 0, 0, row_bytes);
        let pixels = pixels.build().ok_or_else(|| io::Error::other("a picture with no plane"))?;
        Ok((picture, pixels))
    }
}
