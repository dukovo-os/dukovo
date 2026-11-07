// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Charging the kernel memory that programs' objects take to resource
//! domains (`crate::domains`).
//!
//! Each process has an account in its domain. A kernel object a program
//! makes through its process (`process.create`) charges the account with
//! the bytes the kernel uses for it, and gives them back when it goes,
//! wherever its capabilities have travelled by then. Domains count pages,
//! so an account charges its domain a page each time its bytes reach into
//! a new one, and a domain at its limit refuses new objects, as it refuses
//! new threads.
//!
//! An account gives a page back only once its bytes have fallen a whole
//! page below what it holds: it keeps one spare. A process whose bytes go
//! back and forth across a page's edge, as one opening and closing a file
//! with nothing else charged does, would otherwise charge and give back a
//! page of its domain's each time, and every process of the domain, and
//! of the domains above it, would pass the domain's count between their
//! CPUs. A domain's usage is its accounts' bytes, rounded up to pages,
//! and at most one page more for each account.
//!
//! And each thread keeps a stock of bytes charged to its process's account
//! ([`Stock`], ADR 0405), which the files it opens are charged from and
//! the files of its process it closes give back to, so that threads of one
//! process opening and closing files on several CPUs do not pass their
//! account's line between them each time. A stock takes [`BATCH`] bytes
//! from the account when it runs out, keeps at most two batches, and goes
//! back to the account when its thread ends. The account's bytes count
//! what its threads' stocks hold.

use alloc::sync::Arc;
use core::mem::ManuallyDrop;
use core::sync::atomic::{AtomicU64, Ordering};

use radix_abi::errors::Kernel;
use radix_mm::{Flags, HEAP, Kmalloc, PAGE_SIZE};

use super::{Result, error};
use crate::domains::DomainRef;

/// An account, shared by a process and the objects charged to it.
pub type AccountRef = Arc<Account, Kmalloc>;

/// Kernel memory charged to a domain on behalf of one process, on a cache
/// line of its own: every open and close of the process changes its
/// count, and accounts made one after another would otherwise share lines,
/// which their processes on other CPUs would pass between them.
#[repr(align(64))]
pub struct Account {
    domain: DomainRef,
    /// The bytes charged, and [`SPARE`]. Each change is one
    /// compare-and-swap, and the domain is charged or given back the pages
    /// that change moves the count across, so the domain holds the pages
    /// of what the account holds once no change is under way (opening a
    /// file charges and gives back one object's bytes, on its fast path).
    bytes: AtomicU64,
}

/// The bit of an account's `bytes` that says its domain holds a page more
/// for it than its bytes take: the spare it keeps (see the module's
/// documentation). No account charges anywhere near 2^63 bytes.
const SPARE: u64 = 1 << 63;

/// The pages the domain holds for an account whose `bytes` word is `word`.
fn held(word: u64) -> u64 {
    pages(word & !SPARE) + u64::from(word & SPARE != 0)
}

/// Bytes charged to an account, given back when it is dropped: kept by
/// the object they were charged for.
///
/// The charges share one reference to the account between them: the
/// charge that takes the count up from zero makes it, and the one that
/// brings the count back to zero drops it. Each charge with bytes keeps
/// the count above zero, so the account lives as long as it, and opening
/// a file costs no reference count of the account's (two locked
/// instructions). A thread's [`Stock`] keeps the count above zero too,
/// while it holds bytes, but holds no charge: its bytes go back through
/// one when the thread ends.
pub struct Charge {
    /// The account, as a reference that is not counted: dropped only by
    /// the charge that takes the count to zero, in place of the shared one.
    account: ManuallyDrop<AccountRef>,
    bytes: u64,
}

fn pages(bytes: u64) -> u64 {
    bytes.div_ceil(PAGE_SIZE as u64)
}

/// What a thread's [`Stock`] takes from its account when it runs out, and
/// half the most it keeps: a few files' worth.
const BATCH: u64 = 1024;

/// Bytes a thread has charged to its process's account for objects it has
/// yet to make, or took back from objects of the process it closed (see
/// the module's documentation).
///
/// Only its own thread charges from it or gives back to it, in its own
/// system calls, so its count is loaded and stored, never locked: an
/// atomic only so the thread's side may be shared.
pub struct Stock {
    bytes: AtomicU64,
}

impl Stock {
    /// An empty stock.
    pub const fn new() -> Stock {
        Stock { bytes: AtomicU64::new(0) }
    }

    /// Takes back `charge`, of an object the thread closed: into the stock
    /// if it was charged to `account`, the thread's process's, keeping at
    /// most two batches and giving back all but one when it would hold
    /// more; otherwise to its own account. Called by the stock's thread.
    #[inline]
    pub fn take_back(&self, account: &AccountRef, mut charge: Charge) {
        if !Arc::ptr_eq(&charge.account, account) {
            return;
        }
        // The bytes move to the stock, and the account's count, which
        // holds both, stays.
        let have = self.bytes.load(Ordering::Relaxed) + core::mem::take(&mut charge.bytes);
        if have > 2 * BATCH {
            self.bytes.store(BATCH, Ordering::Relaxed);
            // The charge gives the rest back as it is dropped.
            charge.bytes = have - BATCH;
        } else {
            self.bytes.store(have, Ordering::Relaxed);
        }
    }

    /// Gives everything the stock holds back to `account`, the thread's
    /// process's: for a thread that ends. Called by the stock's thread, or
    /// once nothing else can reach it.
    pub fn drain(&self, account: &AccountRef) {
        let bytes = self.bytes.load(Ordering::Relaxed);
        if bytes > 0 {
            self.bytes.store(0, Ordering::Relaxed);
            // Its bytes keep the account's count above zero: a charge of
            // them takes the shared reference along if they are the last.
            drop(Charge::new(account, false, bytes));
        }
    }
}

impl Account {
    /// A new account in `domain`, with nothing charged.
    pub fn new(domain: DomainRef) -> Option<AccountRef> {
        let account = Account { domain, bytes: AtomicU64::new(0) };
        Arc::try_new_in(account, HEAP.with(Flags::MAY_SLEEP)).ok()
    }

    /// The domain it charges.
    pub fn domain(&self) -> &DomainRef {
        &self.domain
    }

    /// The bytes charged now.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed) & !SPARE
    }

    /// Charges `bytes`, unless the domain or one above it would pass its
    /// limit.
    pub fn charge(account: &AccountRef, bytes: u64) -> Result<Charge> {
        let mut word = account.bytes.load(Ordering::Relaxed);
        loop {
            let charged = word & !SPARE;
            let after = charged
                .checked_add(bytes)
                .filter(|after| after & SPARE == 0)
                .ok_or(error(Kernel::NoMemory))?;
            // A spare page takes the new bytes before the domain is asked.
            let more = pages(after).saturating_sub(held(word));
            if more > 0 {
                account.domain.charge(more).map_err(|_| error(Kernel::NoMemory))?;
            }
            // The spare stays one if the new bytes did not reach into it.
            let spare = if held(word) > pages(after) { SPARE } else { 0 };
            match account.bytes.compare_exchange_weak(
                word,
                after | spare,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(Charge::new(account, charged == 0, bytes)),
                Err(now) => {
                    // Another charge or release came first: give back
                    // the pages taken for the count it changed.
                    if more > 0 {
                        account.domain.uncharge(more);
                    }
                    word = now;
                }
            }
        }
    }

    /// Charges `bytes` from `stock`, the running thread's, when `account`
    /// is its process's: the stock takes [`BATCH`] more from the account
    /// when it runs out, unless a domain's limit refuses that, when only
    /// `bytes` are charged.
    #[inline]
    pub fn charge_from(account: &AccountRef, stock: &Stock, bytes: u64) -> Result<Charge> {
        let have = stock.bytes.load(Ordering::Relaxed);
        if have >= bytes {
            stock.bytes.store(have - bytes, Ordering::Relaxed);
            // The stock's bytes kept the count above zero, so the shared
            // reference is made already.
            return Ok(Charge::new(account, false, bytes));
        }
        Account::charge_batch(account, stock, bytes)
    }

    /// [`Account::charge_from`] for a stock that has run out.
    #[inline(never)]
    fn charge_batch(account: &AccountRef, stock: &Stock, bytes: u64) -> Result<Charge> {
        let Ok(mut charge) = Account::charge(account, bytes + BATCH) else {
            return Account::charge(account, bytes);
        };
        // The batch stays in the account's count, held by the stock now.
        charge.bytes = bytes;
        stock.bytes.store(stock.bytes.load(Ordering::Relaxed) + BATCH, Ordering::Relaxed);
        Ok(charge)
    }
}

impl Drop for Account {
    fn drop(&mut self) {
        // Every charge has gone, as each one keeps the account: what the
        // domain still holds for it is the spare page.
        let held = held(*self.bytes.get_mut());
        if held > 0 {
            self.domain.uncharge(held);
        }
    }
}

impl Charge {
    /// A charge of `bytes` to `account`, counted there already; `first`
    /// if it took the count up from zero.
    #[allow(unsafe_code)]
    fn new(account: &AccountRef, first: bool, bytes: u64) -> Charge {
        if first && bytes > 0 {
            // The count left zero: the shared reference is made.
            core::mem::forget(account.clone());
        }
        let alloc = *Arc::allocator(account);
        // SAFETY: the pointer is a live `Arc`'s, with its allocator. The
        // `Arc` made from it is not counted, and is dropped only in place
        // of the shared reference, which a charge with bytes keeps (see
        // `Charge`); one without bytes never drops it.
        let account = unsafe { Arc::from_raw_in(Arc::as_ptr(account), alloc) };
        Charge { account: ManuallyDrop::new(account), bytes }
    }

    /// Takes `other`, charged to the same account, into this charge: both
    /// are given back together, when it is dropped.
    pub fn absorb(&mut self, mut other: Charge) {
        debug_assert!(Arc::ptr_eq(&self.account, &other.account));
        // Both keep the count above zero until here, and after this one
        // keeps it, for both.
        self.bytes += core::mem::take(&mut other.bytes);
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        let bytes = self.bytes;
        if bytes == 0 {
            // It kept nothing: the count, and the shared reference, are
            // other charges'.
            return;
        }
        let mut word = self.account.bytes.load(Ordering::Relaxed);
        loop {
            let after = (word & !SPARE) - bytes;
            // The domain keeps one page more than the bytes left take, if
            // it held one: the spare. The rest goes back.
            let over = held(word) - pages(after);
            let spare = if over > 0 { SPARE } else { 0 };
            match self.account.bytes.compare_exchange_weak(
                word,
                after | spare,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let fewer = over.saturating_sub(1);
                    if fewer > 0 {
                        self.account.domain.uncharge(fewer);
                    }
                    if after == 0 {
                        // The last bytes went: the shared reference goes
                        // with them.
                        // SAFETY: dropped once, here, and not used after.
                        #[allow(unsafe_code)]
                        unsafe {
                            ManuallyDrop::drop(&mut self.account);
                        }
                    }
                    return;
                }
                Err(now) => word = now,
            }
        }
    }
}
