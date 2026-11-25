// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Who may write a file, and whether programs share its pages (ADR 0658).
//!
//! The kernel keeps one copy of the pages programs map from a file no one
//! may write, for every process that maps them (ADR 0563). A write to the
//! file would leave those copies as they were, so the file may not be
//! written while they are kept. A filesystem whose files change only
//! through the kernel's writes counts, for each file, the handles that
//! may write it and the sets of pages kept of it ([`WriteAccess`]), and
//! one of the two is always none:
//!
//! - an open to write or to cut the file takes a [`Writer`], and fails
//!   with [`Error::TextBusy`] while pages are kept, as Linux's open of a
//!   program that runs fails with ETXTBSY;
//! - the kernel keeps pages of the file only while no handle may write
//!   it ([`WriteAccess::deny_writes`]), and otherwise each process fills
//!   its own copies, as for any file.
//!
//! Every change to a file's bytes goes through a handle that may write it:
//! a write, a change of size, a clone into it, or the cut of an open with
//! `O_TRUNC`, which holds its writer from before the cut. Removing,
//! renaming and linking the file change no bytes, and take none.

use core::sync::atomic::{AtomicIsize, Ordering};

use crate::{Error, NodeRef, Result};

/// A file's handles that may write it, counted up from 0, or its sets of
/// kept pages, counted down from 0: the count is never both.
#[derive(Debug, Default)]
pub struct WriteAccess(AtomicIsize);

impl WriteAccess {
    /// A file no one writes and no program shares the pages of.
    pub const fn new() -> WriteAccess {
        WriteAccess(AtomicIsize::new(0))
    }

    /// One more handle that may write, unless pages are kept.
    fn begin_write(&self) -> Result<()> {
        let mut now = self.0.load(Ordering::Relaxed);
        loop {
            if now < 0 {
                return Err(Error::TextBusy);
            }
            match self.0.compare_exchange_weak(now, now + 1, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return Ok(()),
                Err(seen) => now = seen,
            }
        }
    }

    /// One handle that may write fewer.
    fn end_write(&self) {
        self.0.fetch_sub(1, Ordering::Release);
    }

    /// One more set of kept pages, unless a handle may write: whether
    /// pages may be kept. The caller lets go with
    /// [`WriteAccess::allow_writes`].
    pub fn deny_writes(&self) -> bool {
        let mut now = self.0.load(Ordering::Relaxed);
        loop {
            if now > 0 {
                return false;
            }
            match self.0.compare_exchange_weak(now, now - 1, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return true,
                Err(seen) => now = seen,
            }
        }
    }

    /// One set of kept pages fewer, which [`WriteAccess::deny_writes`]
    /// allowed.
    pub fn allow_writes(&self) {
        self.0.fetch_add(1, Ordering::Release);
    }

    /// Whether pages of the file are kept: an open to write it would fail.
    pub fn denied(&self) -> bool {
        self.0.load(Ordering::Acquire) < 0
    }
}

/// A handle's leave to write a file, from the open that made the handle
/// until the handle goes.
pub struct Writer {
    /// The file, which the writer keeps alive and so its count.
    node: NodeRef,
}

impl Writer {
    /// A writer of `node`: `None` if its filesystem counts none, and
    /// [`Error::TextBusy`] while programs share its pages.
    pub fn of(node: &NodeRef) -> Result<Option<Writer>> {
        let Some(access) = node.write_access() else {
            return Ok(None);
        };
        access.begin_write()?;
        Ok(Some(Writer { node: node.clone() }))
    }

    /// One more writer of the same file, for a second handle that writes
    /// as this one does. It cannot fail: while this one lives, no pages
    /// of the file are kept.
    pub fn again(&self) -> Writer {
        if let Some(access) = self.node.write_access() {
            access.0.fetch_add(1, Ordering::AcqRel);
        }
        Writer { node: self.node.clone() }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if let Some(access) = self.node.write_access() {
            access.end_write();
        }
    }
}
