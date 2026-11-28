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

/// `address` and `len` if the `len` bytes from `address` end at or below
/// `top`, and `top` and 0 if they do not (their end past `top`, or past
/// the top of the address space), with no branch the processor could
/// guess: take it after the range check, and copy with what it gives.
/// `top` must be an address nothing is ever mapped at, so that a copy the
/// processor runs ahead with reads nothing: from `top` it copies nothing,
/// and a byte or a word just below it is the caller's own.
///
/// This is what Linux's `mask_user_address` does for a copy from or to a
/// program, where a fence after the check would wait for everything
/// before it to finish (ADR 0803).
#[inline(always)]
pub fn range(address: u64, len: u64, top: u64) -> (u64, u64) {
    let keep = range_mask(address, len, top);
    ((address & keep) | (top & !keep), len & keep)
}

/// All ones if the `len` bytes from `address` end at or below `top`, and 0
/// if they do not.
#[inline(always)]
fn range_mask(address: u64, len: u64, top: u64) -> u64 {
    #[cfg(all(target_arch = "x86_64", not(kani)))]
    {
        let keep: u64;
        // SAFETY: adds, compares and subtracts with borrow, of registers
        // only: the first borrow is the end's carry past the top of the
        // address space, the second whether the end is above `top`, and
        // each subtract spreads its borrow across a register. `keep` is
        // written while the inputs are still read, so it is a register of
        // its own (`out`, not `lateout`). The block is not `pure`, so the
        // compiler keeps it where it stands, after the check.
        unsafe {
            core::arch::asm!(
                "add {end}, {len}",
                "sbb {keep}, {keep}",
                "cmp {top}, {end}",
                "sbb {len}, {len}",
                "or {keep}, {len}",
                "not {keep}",
                end = inout(reg) address => _,
                len = inout(reg) len => _,
                top = in(reg) top,
                keep = out(reg) keep,
                options(nomem, nostack),
            );
        }
        keep
    }
    #[cfg(not(all(target_arch = "x86_64", not(kani))))]
    portable_range_mask(address, len, top)
}

/// [`range_mask`] in arithmetic alone.
#[cfg(any(test, not(all(target_arch = "x86_64", not(kani)))))]
#[inline(always)]
fn portable_range_mask(address: u64, len: u64, top: u64) -> u64 {
    let (end, wrapped) = address.overflowing_add(len);
    let wrapped = u64::from(wrapped).wrapping_neg();
    let past = u64::from(top < end).wrapping_neg();
    !(wrapped | past)
}

#[cfg(test)]
mod tests {
    use super::{index, mask, portable_mask, portable_range_mask, range, range_mask};

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

    #[test]
    fn ranges_within_the_top_stay_and_others_become_nothing_at_the_top() {
        const TOP: u64 = 0x0000_7fff_ffff_f000;
        let addresses = [0, 1, 0x10000, TOP - 64, TOP - 1, TOP, TOP + 1, u64::MAX - 1, u64::MAX];
        let lens = [0, 1, 8, 63, 64, 4096, TOP, u64::MAX - 1, u64::MAX];
        for address in addresses {
            for len in lens {
                let fits = address.checked_add(len).is_some_and(|end| end <= TOP);
                let want = if fits { (address, len) } else { (TOP, 0) };
                assert_eq!(range(address, len, TOP), want, "{len} bytes from {address:#x}");
                assert_eq!(
                    portable_range_mask(address, len, TOP),
                    range_mask(address, len, TOP),
                    "{len} bytes from {address:#x}"
                );
            }
        }
    }
}
