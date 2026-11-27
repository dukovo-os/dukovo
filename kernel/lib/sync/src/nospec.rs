// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Indices a program chooses, kept in bounds while the processor runs
//! ahead (Spectre v1, ADR 0790).
//!
//! A processor guesses which way a bounds check goes before it knows, and
//! runs ahead with the guess: an index past the end then reads memory the
//! check was there to keep it from, and what it read can be seen in the
//! cache after the guess is undone. [`index`], after the check, gives the
//! index where it is in bounds and 0 where it is not, through arithmetic
//! rather than another branch, so the processor reads no further while it
//! runs ahead, as Linux's `array_index_nospec`. The kernel uses it on every
//! index a program names before it reads an array with it: a system call's
//! number, a capability slot, a signal, a resource limit.

/// `index` if it is below `len`, and 0 if it is not, with no branch the
/// processor could guess: take it after the bounds check, and index with
/// what it gives.
#[inline(always)]
pub fn index(index: usize, len: usize) -> usize {
    index & mask(index, len)
}

/// All ones if `index` is below `len`, and 0 if it is not.
#[inline(always)]
fn mask(index: usize, len: usize) -> usize {
    #[cfg(all(target_arch = "x86_64", not(kani)))]
    {
        let mask: usize;
        // SAFETY: a compare and a subtract with borrow, of registers only:
        // the borrow is set where `index` is below `len`, and the subtract
        // spreads it across `mask`. The block is not `pure`, so the
        // compiler keeps it where it stands, after the check.
        unsafe {
            core::arch::asm!(
                "cmp {index}, {len}",
                "sbb {mask}, {mask}",
                index = in(reg) index,
                len = in(reg) len,
                mask = lateout(reg) mask,
                options(nomem, nostack),
            );
        }
        mask
    }
    #[cfg(not(all(target_arch = "x86_64", not(kani))))]
    portable_mask(index, len)
}

/// [`mask`] in arithmetic alone, as Linux's generic form: the top bit of
/// `index | (len - 1 - index)` is clear only where `index` is below `len`
/// (`len` under 2^63).
#[cfg(any(test, not(all(target_arch = "x86_64", not(kani)))))]
#[inline(always)]
fn portable_mask(index: usize, len: usize) -> usize {
    let gap = len.wrapping_sub(1).wrapping_sub(index);
    !((index | gap) as isize >> (usize::BITS - 1)) as usize
}

#[cfg(test)]
mod tests {
    use super::{index, mask, portable_mask};

    #[test]
    fn indices_in_bounds_stay_and_others_are_zero() {
        for len in [0, 1, 2, 12, 4096, usize::MAX >> 1] {
            for at in [0, 1, 2, 11, 12, 13, 4095, 4096, usize::MAX >> 1, usize::MAX] {
                let want = if at < len { at } else { 0 };
                assert_eq!(index(at, len), want, "{at} in {len}");
                assert_eq!(portable_mask(at, len), mask(at, len), "{at} in {len}");
            }
        }
    }
}
