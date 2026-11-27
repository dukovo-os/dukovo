// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An allocator that counts the bytes it has out: [`Counted`] passes
//! every call on to the allocator it is made over, and keeps the sum of
//! the sizes asked for and not yet given back.
//!
//! A test measures what a structure keeps with it, where a bar says how
//! much that may be (the display layer's holders and screens, ADR 0700).
//! It is here because an allocator cannot be written in a crate that
//! forbids unsafe code, which the libraries that are measured do (ADR
//! 0730): the trait is `unsafe` to implement, whatever the allocator
//! does.

#![allow(unsafe_code)]

use core::alloc::{AllocError, Allocator, Layout};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};

/// The allocator `A`, with the bytes it has out counted in `out`.
///
/// The count is of the sizes callers asked for, not of what `A` rounds
/// them up to. Copies of a `Counted` count in the same place.
#[derive(Clone, Copy, Debug)]
pub struct Counted<'a, A> {
    inner: A,
    out: &'a AtomicUsize,
}

impl<'a, A> Counted<'a, A> {
    /// Counts what is asked of `inner` in `out`, which the caller reads.
    pub const fn new(inner: A, out: &'a AtomicUsize) -> Self {
        Self { inner, out }
    }
}

// SAFETY: every block is `inner`'s, which each call is passed on to with
// the layout it came with; the count kept beside them changes nothing of
// them. A copy holds a copy of `inner`, which by `A`'s own contract is the
// same allocator.
unsafe impl<A: Allocator> Allocator for Counted<'_, A> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let block = self.inner.allocate(layout)?;
        self.out.fetch_add(layout.size(), Ordering::Relaxed);
        Ok(block)
    }

    unsafe fn deallocate(&self, block: NonNull<u8>, layout: Layout) {
        self.out.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: what the caller promises this allocator is what `inner`
        // asks: `block` came from `allocate` with `layout`, which passed
        // it on from `inner`.
        unsafe { self.inner.deallocate(block, layout) };
    }
}

#[cfg(test)]
mod tests {
    use alloc::alloc::Global;
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::Counted;

    #[test]
    fn the_count_is_what_is_out_and_copies_share_it() {
        let out = AtomicUsize::new(0);
        let counted = Counted::new(Global, &out);
        let mut numbers: Vec<u64, _> = Vec::with_capacity_in(4, counted);
        assert_eq!(out.load(Ordering::Relaxed), 32);
        // A vector that grows gives its old block back.
        numbers.extend(0..5);
        let grown = out.load(Ordering::Relaxed);
        assert_eq!(grown, numbers.capacity() * 8);
        let boxed = Box::new_in([0u8; 100], counted);
        assert_eq!(out.load(Ordering::Relaxed), grown + 100);
        drop(numbers);
        assert_eq!(out.load(Ordering::Relaxed), 100);
        drop(boxed);
        assert_eq!(out.load(Ordering::Relaxed), 0);
    }
}
