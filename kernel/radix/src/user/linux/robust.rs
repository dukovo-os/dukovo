// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Robust futex lists (ADR 0617): what a Linux thread's end, or its
//! `exec`, does to the locks it holds.
//!
//! `set_robust_list` names a list, in the thread's memory, of the robust
//! locks it holds (a C library's robust mutexes): a `struct
//! robust_list_head` of the list's first entry, the offset from each entry
//! to its lock's word, and the entry being added or taken off
//! (`list_op_pending`). When the thread ends, or runs `exec`, the kernel
//! walks the list as Linux's `exit_robust_list` does: each word the thread
//! still owns gets FUTEX_OWNER_DIED, keeping FUTEX_WAITERS, and if it had
//! waiters one of them wakes, to find the owner gone (the C library's
//! EOWNERDEAD); the pending entry comes last
//! (`radix_linux::futex::death`). At most ROBUST_LIST_MAX entries are
//! looked at, so that a list that loops ends. An entry or a word that
//! cannot be read, a word that cannot be written, or one out of line with
//! 4 bytes ends the walk, as on Linux. An entry's low bit marks a priority
//! inheritance lock, which the kernel does not have (docs/deferred.md):
//! its word is marked, and no waiter woken for it.

use core::sync::atomic::Ordering;

use radix_abi::address_space::{FUTEX_ALL, ROBUST_LIST_MAX};
use radix_hal::UserMode;
use radix_linux::futex::{Death, FUTEX_WAITERS, death};

use crate::Arch;
use crate::sched::ThreadRef;
use crate::user::UserThread;
use crate::user::memory::{SpaceObject, futex};

/// Run by a program's thread, `thread`, as it ends or runs `exec`: walks
/// its robust list, if it has one, and forgets it, as Linux does.
pub fn ending(thread: &ThreadRef, user: &UserThread) {
    let head = user.linux.robust_list.swap(0, Ordering::Relaxed);
    if head != 0 {
        let _ = walk(user.space(), head, super::state::tid(thread));
    }
}

/// Walks the list whose head is at `head` for the thread numbered `tid`;
/// `None` where a fault, or a word out of line, ended it.
fn walk(space: &SpaceObject, head: u64, tid: u32) -> Option<()> {
    let (mut entry, mut pi) = entry_at(head)?;
    let offset = read(head.checked_add(8)?)?;
    let (pending, pending_pi) = entry_at(head.checked_add(16)?)?;
    let mut left = ROBUST_LIST_MAX;
    while entry != head {
        // The next entry is read before this one's lock is marked: a
        // thread that takes the lock then may change the entry.
        let next = entry_at(entry);
        // The pending entry may be on the list too: it is done once, last.
        if entry != pending {
            end(space, entry.wrapping_add(offset), tid, pi, false)?;
        }
        (entry, pi) = next?;
        left -= 1;
        if left == 0 {
            break;
        }
    }
    if pending != 0 {
        end(space, pending.wrapping_add(offset), tid, pending_pi, true)?;
    }
    Some(())
}

/// The entry a list pointer at `address` points to, and whether its low
/// bit marks a priority inheritance lock.
fn entry_at(address: u64) -> Option<(u64, bool)> {
    let raw = read(address)?;
    Some((raw & !1, raw & 1 != 0))
}

/// The 64-bit word of the thread's memory at `address`.
fn read(address: u64) -> Option<u64> {
    let mut bytes = [0u8; 8];
    Arch::copy_from_user(&mut bytes, address).then(|| u64::from_le_bytes(bytes))
}

/// What the end of the thread numbered `tid` does to the lock word at
/// `address` of `space`, its own: `pi` for a priority inheritance lock,
/// `pending` for the list's pending entry.
fn end(space: &SpaceObject, address: u64, tid: u32, pi: bool, pending: bool) -> Option<()> {
    if !address.is_multiple_of(4) {
        return None;
    }
    loop {
        let mut bytes = [0u8; 4];
        if !Arch::copy_from_user(&mut bytes, address) {
            return None;
        }
        let word = u32::from_le_bytes(bytes);
        match death(word, tid, pi, pending) {
            Death::Leave => return Some(()),
            Death::Wake => {
                wake_one(space, address);
                return Some(());
            }
            Death::Mark(marked) => {
                if Arch::user_compare_exchange(address, word, marked)? != word {
                    // Changed since it was read: look again.
                    continue;
                }
                if !pi && word & FUTEX_WAITERS != 0 {
                    wake_one(space, address);
                }
                return Some(());
            }
        }
    }
}

/// Wakes one waiter on the word at `address` of `space`, which is aligned.
fn wake_one(space: &SpaceObject, address: u64) {
    let _ = futex::wake(space, address, 1, FUTEX_ALL);
}
