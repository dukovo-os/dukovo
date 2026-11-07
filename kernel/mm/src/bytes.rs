// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Vectors filled at the speed of a string store: bytes, and the arrays
//! of plain values the kernel makes for each process. The kernel's
//! `alloc` has no `resize` or `extend_from_slice`, which cannot fail (ADR
//! 0017), and pushing a value at a time costs several instructions each:
//! a 4 KiB buffer took some 28,000 of them in `exec`, and a fork spent
//! some 3,500 pushing its child's arguments and signal tables.

#![allow(unsafe_code)]

use alloc::collections::TryReserveError;
use alloc::vec::Vec;
use core::alloc::Allocator;
use core::mem::MaybeUninit;

/// A vector of `len` zero bytes from `alloc`.
pub fn zeroed<A: Allocator>(len: usize, alloc: A) -> Result<Vec<u8, A>, TryReserveError> {
    filled(0, len, alloc)
}

/// A vector of `len` copies of `value` from `alloc`: what `vec![value;
/// len]` makes. Inlined, so that a value of zeros becomes a string store.
#[inline]
pub fn filled<T: Copy, A: Allocator>(
    value: T,
    len: usize,
    alloc: A,
) -> Result<Vec<T, A>, TryReserveError> {
    let mut values = Vec::new_in(alloc);
    values.try_reserve_exact(len)?;
    values.spare_capacity_mut()[..len].fill(MaybeUninit::new(value));
    // SAFETY: there was room for `len` values, and each was just written.
    unsafe { values.set_len(len) };
    Ok(values)
}

/// Appends `more` to `values`, growing it if need be: one copy of the
/// whole slice, as `extend_from_slice` makes.
pub fn extend<T: Copy, A: Allocator>(
    values: &mut Vec<T, A>,
    more: &[T],
) -> Result<(), TryReserveError> {
    values.try_reserve(more.len())?;
    let len = values.len();
    values.spare_capacity_mut()[..more.len()].write_copy_of_slice(more);
    // SAFETY: there was room for `more`, and each of its values was just
    // written after the `len` there were.
    unsafe { values.set_len(len + more.len()) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::alloc::Global;

    use super::*;

    #[test]
    fn zeroed_is_all_zeros() {
        for len in [0, 1, 7, 4096, 65537] {
            let bytes = zeroed(len, Global).unwrap();
            assert_eq!(bytes.len(), len);
            assert!(bytes.iter().all(|&b| b == 0));
        }
    }

    #[test]
    fn extend_appends() {
        let mut bytes = zeroed(3, Global).unwrap();
        extend(&mut bytes, b"abc").unwrap();
        extend(&mut bytes, b"").unwrap();
        let long: Vec<u8> = (0..5000u32).map(|i| i as u8).collect();
        extend(&mut bytes, &long).unwrap();
        assert_eq!(&bytes[..6], b"\0\0\0abc");
        assert_eq!(&bytes[6..], &long[..]);
    }

    #[test]
    fn filled_repeats_the_value() {
        for len in [0, 1, 65, 1000] {
            let values = filled((7u32, 9u64), len, Global).unwrap();
            assert_eq!(values.len(), len);
            assert!(values.iter().all(|&value| value == (7, 9)));
        }
    }

    #[test]
    fn extend_copies_other_values() {
        let mut values = filled(1u64, 2, Global).unwrap();
        extend(&mut values, &[2, 3]).unwrap();
        extend(&mut values, &[]).unwrap();
        assert_eq!(values, [1, 1, 2, 3]);
    }

    #[test]
    fn too_much_is_an_error() {
        assert!(zeroed(usize::MAX, Global).is_err());
        assert!(filled(0u64, usize::MAX, Global).is_err());
    }
}
