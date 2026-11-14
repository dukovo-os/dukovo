// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Resource domains: groups of threads that share a CPU weight and a
//! memory limit, after Linux's cgroups (v2), and their class for the
//! out-of-memory policy (spec 4.1).
//!
//! Domains form a tree under the root domain, `system`, which holds the
//! kernel's own threads. Each domain has:
//!
//! - a CPU weight, 1 to 10,000 (default 100): on each CPU, the fair class
//!   splits a domain's time between its own threads and its child domains
//!   in proportion to their weights, its own threads together counting as
//!   one child of the default weight (`sched::queue`);
//! - a memory limit, in pages, that its charges and its children's
//!   together may not pass: a charge that would is refused and the
//!   allocation fails with an error for the caller, never a kill;
//! - a thread limit, that its threads and its children's together may not
//!   pass, as Linux's `pids.max`: a thread that would is not made, and
//!   `fork` or `pthread_create` fails with EAGAIN (ADR 0476);
//! - what its threads have read from disks and written to them
//!   ([`DiskUse`], ADR 0476): counted in the domain alone as it goes, so
//!   that a request costs no more than its own domain's counters, and
//!   summed over the domains below it when asked; a domain that goes
//!   hands its counts to its parent;
//! - a disk I/O weight, 1 to 10,000 (default 100), and rates of bytes and
//!   requests each second, read and written (ADR 0476). A domain's rates
//!   hold its threads' requests and those of the domains below it, across
//!   all disks together, as Linux's `io.max` holds a cgroup's on each
//!   disk: a thread waits before a request until the slowest allows it.
//!   While any domain has a weight other than the default, each disk
//!   serves the domains whose requests wait for it in proportion to their
//!   weights, flattened along the tree ([`Domain::flat_io_weight`]), as
//!   `io.weight` shares a disk out;
//! - an out-of-memory class, [`OomClass`]: when the machine as a whole runs
//!   out of memory, `oom` stops whole domains, apps first and services
//!   next, and never `system` ones, which hold the trusted computing base;
//! - optionally a supervisor in the kernel, which restarts the domain
//!   after an out-of-memory stop on the `supervisor` thread (`oom`). The
//!   domains programs make through capabilities (radix-resource-domain(2),
//!   Phase 9) have none: Spark, which makes one for each service it
//!   starts, restarts the service itself once the domain's last thread has
//!   gone, which lets the domain take threads again.
//!
//! Domains are kernel objects that capabilities name
//! ([`radix_cap::ObjectKind::ResourceDomain`]); `caps::create_domain` makes
//! one with its first capability. Threads are charged for their stacks,
//! and the objects a program makes with `process.create` are charged to
//! its process's domain (`user::account`); Phase 20 builds containers on
//! domains.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use radix_block::share::Rate;
use radix_cap::ObjectId;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sched::{DOMAIN_WEIGHT_DEFAULT, DOMAIN_WEIGHT_MAX};
use radix_sync::{Once, SpinLock};

use crate::sched::Name;

/// A domain, shared.
pub type DomainRef = Arc<Domain, Kmalloc>;

/// Deepest domain below the root.
pub const MAX_DEPTH: u8 = 4;

/// Most domains at once.
pub const MAX_DOMAINS: usize = 256;

/// The heaviest disk I/O weight.
pub const IO_WEIGHT_MAX: u32 = 10_000;

/// The disk I/O weight a domain has unless it is given another.
pub const IO_WEIGHT_DEFAULT: u32 = 100;

/// The rates a domain's requests keep to across all disks together, each
/// second ([`Domain::set_io`]); `u64::MAX` for none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoRates {
    /// Bytes read.
    pub read_bps: u64,
    /// Bytes written, those a write leaves dirty in the page cache too.
    pub write_bps: u64,
    /// Read requests.
    pub read_iops: u64,
    /// Write requests.
    pub write_iops: u64,
}

impl IoRates {
    /// No rate at all.
    pub const NONE: IoRates = IoRates {
        read_bps: u64::MAX,
        write_bps: u64::MAX,
        read_iops: u64::MAX,
        write_iops: u64::MAX,
    };

    /// Whether any rate is set.
    pub fn any(&self) -> bool {
        *self != IoRates::NONE
    }
}

/// What the out-of-memory policy may do to a domain, least protected
/// first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OomClass {
    /// A user app: stopped first.
    App,
    /// A system service: stopped once no app is left to stop, and
    /// restarted by its supervisor.
    Service,
    /// The kernel and the trusted computing base: never stopped. An
    /// allocation fails instead (spec 3.4 and 16.1).
    System,
}

impl OomClass {
    /// Lower-case name.
    pub const fn name(self) -> &'static str {
        match self {
            OomClass::App => "app",
            OomClass::Service => "service",
            OomClass::System => "system",
        }
    }
}

/// What a new domain should be.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// CPU weight, 1 to 10,000.
    pub weight: u32,
    /// Memory limit in pages; `None` for none beyond the parent's.
    pub limit_pages: Option<u64>,
    /// Out-of-memory class; no more protected than the parent's.
    pub class: OomClass,
    /// What restarts it after an out-of-memory stop, if anything.
    pub supervisor: Option<fn(&DomainRef)>,
    /// Thread limit; `None` for none beyond the parent's.
    pub thread_limit: Option<u32>,
}

impl Settings {
    /// Default weight, no limits, in `class`, unsupervised.
    pub const fn new(class: OomClass) -> Settings {
        Settings {
            weight: DOMAIN_WEIGHT_DEFAULT,
            limit_pages: None,
            class,
            supervisor: None,
            thread_limit: None,
        }
    }
}

/// What a domain's threads, and those of the domains below it, moved to
/// and from disks: the bytes and requests the devices finished, and the
/// time requests waited for the device's room and for the domain's rates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskUse {
    /// Bytes read.
    pub read_bytes: u64,
    /// Bytes written: those a write left dirty in the page cache, counted
    /// as it did, and those written to a disk directly.
    pub written_bytes: u64,
    /// Read requests.
    pub reads: u64,
    /// Write requests written directly.
    pub writes: u64,
    /// Nanoseconds requests waited for room on a device.
    pub wait_ns: u64,
    /// Nanoseconds requests and writers were held to the domain's rates.
    pub throttled_ns: u64,
}

impl DiskUse {
    fn add(&mut self, other: &DiskUse) {
        self.read_bytes += other.read_bytes;
        self.written_bytes += other.written_bytes;
        self.reads += other.reads;
        self.writes += other.writes;
        self.wait_ns += other.wait_ns;
        self.throttled_ns += other.throttled_ns;
    }
}

/// [`DiskUse`] as a domain counts it, its own and that of the domains
/// below it that have gone.
#[derive(Default)]
struct DiskCounts {
    read_bytes: AtomicU64,
    written_bytes: AtomicU64,
    reads: AtomicU64,
    writes: AtomicU64,
    wait_ns: AtomicU64,
    throttled_ns: AtomicU64,
}

impl DiskCounts {
    fn read(&self) -> DiskUse {
        DiskUse {
            read_bytes: self.read_bytes.load(Ordering::Relaxed),
            written_bytes: self.written_bytes.load(Ordering::Relaxed),
            reads: self.reads.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            wait_ns: self.wait_ns.load(Ordering::Relaxed),
            throttled_ns: self.throttled_ns.load(Ordering::Relaxed),
        }
    }

    fn add(&self, used: &DiskUse) {
        self.read_bytes.fetch_add(used.read_bytes, Ordering::Relaxed);
        self.written_bytes.fetch_add(used.written_bytes, Ordering::Relaxed);
        self.reads.fetch_add(used.reads, Ordering::Relaxed);
        self.writes.fetch_add(used.writes, Ordering::Relaxed);
        self.wait_ns.fetch_add(used.wait_ns, Ordering::Relaxed);
        self.throttled_ns.fetch_add(used.throttled_ns, Ordering::Relaxed);
    }
}

/// Why a thread could not join a domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinError {
    /// The domain is being stopped.
    Stopping,
    /// The domain, or one above it, has as many threads as its limit.
    TooManyThreads,
}

/// Why a domain operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainError {
    /// The charge would pass the limit of this domain or one above it.
    OverLimit,
    /// A weight out of range, a class more protected than the parent's,
    /// or a tree deeper than [`MAX_DEPTH`].
    InvalidArgument,
    /// No memory, or [`MAX_DOMAINS`] domains exist already.
    NoMemory,
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DomainError::OverLimit => "the domain's memory limit is reached",
            DomainError::InvalidArgument => "invalid domain settings",
            DomainError::NoMemory => "out of memory for domains",
        })
    }
}

/// A resource domain.
pub struct Domain {
    id: u32,
    name: Name,
    parent: Option<DomainRef>,
    depth: u8,
    class: OomClass,
    supervisor: Option<fn(&DomainRef)>,
    /// The capability object naming it, if it was made through one.
    object: Option<ObjectId>,
    weight: AtomicU32,
    limit: AtomicU64,
    /// Pages charged to it and its children.
    usage: AtomicU64,
    peak: AtomicU64,
    refused: AtomicU64,
    /// Its threads, in the low 32 bits, and [`STOPPING`]: one word, so
    /// that no thread joins a domain being stopped and the last thread to
    /// leave clears the mark, with no moment in between for the other
    /// side to come in.
    state: AtomicU64,
    oom_stops: AtomicU32,
    /// Its threads and those of the domains below it.
    within: AtomicU32,
    /// The most threads `within` may count; `u32::MAX` for no limit.
    thread_limit: AtomicU32,
    /// What its own threads moved to and from disks, and what the
    /// domains below it that have gone did.
    disk: DiskCounts,
    /// Its disk I/O weight, 1 to [`IO_WEIGHT_MAX`].
    io_weight: AtomicU32,
    /// Its rates, as [`IoRates`] lists them, `None` for none: what each
    /// allows and has been taken of it.
    io_rates: SpinLock<[Option<Rate>; 4]>,
}

/// How many domains have a disk I/O weight other than the default: while
/// none has, the disks serve requests in the order they come.
static WEIGHTED: AtomicU32 = AtomicU32::new(0);

/// How many domains have a disk I/O rate: while none has, requests take
/// nothing from any.
static RATED: AtomicU32 = AtomicU32::new(0);

/// Whether a domain has a disk I/O weight other than the default.
pub fn io_weighted() -> bool {
    WEIGHTED.load(Ordering::Relaxed) > 0
}

/// Whether a domain has a disk I/O rate.
pub fn io_rated() -> bool {
    RATED.load(Ordering::Relaxed) > 0
}

/// Counts in `counter` a domain that is now counted (`now`) or not, and
/// was (`was`) or not.
fn recount(counter: &AtomicU32, was: bool, now: bool) {
    match (was, now) {
        (false, true) => {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        (true, false) => {
            counter.fetch_sub(1, Ordering::Relaxed);
        }
        _ => {}
    }
}

impl Drop for Domain {
    /// Hands what it moved to and from disks to its parent, so that the
    /// parent's counts never go back.
    fn drop(&mut self) {
        if let Some(parent) = &self.parent {
            parent.disk.add(&self.disk.read());
        }
        recount(&WEIGHTED, *self.io_weight.get_mut() != IO_WEIGHT_DEFAULT, false);
        recount(&RATED, self.io_rates.get_mut().iter().any(Option::is_some), false);
    }
}

/// The bit of [`Domain`]'s state that says it is being stopped.
const STOPPING: u64 = 1 << 32;

/// The bits of [`Domain`]'s state that count its threads.
const THREAD_COUNT: u64 = STOPPING - 1;

impl Domain {
    /// Its number, unique for as long as the kernel runs.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Its name.
    pub fn name(&self) -> &Name {
        &self.name
    }

    /// The domain above it; `None` for the root.
    pub fn parent(&self) -> Option<&DomainRef> {
        self.parent.as_ref()
    }

    /// How far below the root it is.
    pub fn depth(&self) -> u8 {
        self.depth
    }

    /// Its out-of-memory class.
    pub fn class(&self) -> OomClass {
        self.class
    }

    /// What restarts it after an out-of-memory stop.
    pub fn supervisor(&self) -> Option<fn(&DomainRef)> {
        self.supervisor
    }

    /// Its CPU weight.
    pub fn weight(&self) -> u32 {
        self.weight.load(Ordering::Relaxed)
    }

    /// Changes its CPU weight; it applies when the run queues next
    /// recompute their shares.
    pub fn set_weight(&self, weight: u32) -> Result<(), DomainError> {
        if !(1..=DOMAIN_WEIGHT_MAX).contains(&weight) {
            return Err(DomainError::InvalidArgument);
        }
        self.weight.store(weight, Ordering::Relaxed);
        Ok(())
    }

    /// Its memory limit in pages (`u64::MAX` for none).
    pub fn limit_pages(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }

    /// Changes its memory limit. A limit below what is charged already
    /// refuses new charges until enough is given back.
    pub fn set_limit_pages(&self, pages: Option<u64>) {
        self.limit.store(pages.unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// Pages charged to it and its children.
    pub fn usage_pages(&self) -> u64 {
        self.usage.load(Ordering::Relaxed)
    }

    /// The most pages it has had charged at once.
    pub fn peak_pages(&self) -> u64 {
        self.peak.load(Ordering::Relaxed)
    }

    /// Charges it refused for its limit (or a limit above it).
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// Threads in it now.
    pub fn threads(&self) -> u32 {
        (self.state.load(Ordering::Relaxed) & THREAD_COUNT) as u32
    }

    /// Threads in it and the domains below it now, which its thread limit
    /// counts.
    pub fn threads_within(&self) -> u32 {
        self.within.load(Ordering::Relaxed)
    }

    /// Its thread limit (`u32::MAX` for none).
    pub fn thread_limit(&self) -> u32 {
        self.thread_limit.load(Ordering::Relaxed)
    }

    /// Changes its thread limit. A limit below the threads it has already
    /// refuses new threads until enough have exited; none is stopped.
    pub fn set_thread_limit(&self, limit: Option<u32>) {
        self.thread_limit.store(limit.unwrap_or(u32::MAX), Ordering::Relaxed);
    }

    /// Its disk I/O weight.
    pub fn io_weight(&self) -> u32 {
        self.io_weight.load(Ordering::Relaxed)
    }

    /// Its disk I/O weight as the disks share them out: its own, made a
    /// hundredth larger or smaller for each hundredth that each domain's
    /// above it is over or under the default. Domains below one parent so
    /// share in proportion to their own weights, and the domains below
    /// one of twice the weight have twice as much between them as those
    /// below its sibling, when as many of each use the disk.
    pub fn flat_io_weight(&self) -> u64 {
        let mut weight = u64::from(self.io_weight());
        let mut above = self.parent.as_deref();
        while let Some(domain) = above {
            weight =
                weight.saturating_mul(u64::from(domain.io_weight())) / u64::from(IO_WEIGHT_DEFAULT);
            above = domain.parent.as_deref();
        }
        weight.max(1)
    }

    /// The rates its requests keep to across all disks together.
    pub fn io_rates(&self) -> IoRates {
        let [read_bps, write_bps, read_iops, write_iops] =
            self.io_rates.lock_irq().map(|rate| rate.map_or(u64::MAX, |r| r.per_second()));
        IoRates { read_bps, write_bps, read_iops, write_iops }
    }

    /// Takes `bytes` in `requests` requests, read, or written (`written`),
    /// from its rates and those of each domain above it, at `now_ns` on
    /// the kernel's clock: how long the thread that is to make them waits
    /// first, in nanoseconds, to keep to the slowest of them.
    pub fn take_io(&self, written: bool, bytes: u64, requests: u64, now_ns: u64) -> u64 {
        // As IoRates lists them: bytes read, then written, then requests.
        let (by_bytes, by_requests) = if written { (1, 3) } else { (0, 2) };
        let mut wait = 0;
        let mut domain = Some(self);
        while let Some(this) = domain {
            let mut rates = this.io_rates.lock_irq();
            for (rate, units) in [(by_bytes, bytes), (by_requests, requests)] {
                if let Some(rate) = &mut rates[rate]
                    && units > 0
                {
                    wait = wait.max(rate.take(units, now_ns));
                }
            }
            drop(rates);
            domain = this.parent.as_deref();
        }
        wait
    }

    /// Changes its disk I/O weight and rates, for the requests made from
    /// now on: a weight out of range, or a rate of 0, is refused.
    pub fn set_io(&self, weight: u32, rates: IoRates) -> Result<(), DomainError> {
        let IoRates { read_bps, write_bps, read_iops, write_iops } = rates;
        if !(1..=IO_WEIGHT_MAX).contains(&weight)
            || [read_bps, write_bps, read_iops, write_iops].contains(&0)
        {
            return Err(DomainError::InvalidArgument);
        }
        // Under its rates' lock, so that two changes at once count it
        // once.
        let mut held = self.io_rates.lock_irq();
        let old = self.io_weight.swap(weight, Ordering::Relaxed);
        recount(&WEIGHTED, old != IO_WEIGHT_DEFAULT, weight != IO_WEIGHT_DEFAULT);
        let was_rated = held.iter().any(Option::is_some);
        for (rate, value) in held.iter_mut().zip([read_bps, write_bps, read_iops, write_iops]) {
            *rate = match (rate.take(), value) {
                (_, u64::MAX) => None,
                // What was taken stays taken at the new rate.
                (Some(mut kept), value) => {
                    kept.set(value);
                    Some(kept)
                }
                (None, value) => Some(Rate::new(value)),
            };
        }
        recount(&RATED, was_rated, rates.any());
        Ok(())
    }

    /// Counts `bytes` read from a disk, in `requests` requests, by one of
    /// its threads, or written (`written`) by one: to a disk directly, or
    /// left dirty in the page cache, with no requests then.
    pub fn count_disk(&self, written: bool, bytes: u64, requests: u64) {
        if written {
            self.disk.written_bytes.fetch_add(bytes, Ordering::Relaxed);
            self.disk.writes.fetch_add(requests, Ordering::Relaxed);
        } else {
            self.disk.read_bytes.fetch_add(bytes, Ordering::Relaxed);
            self.disk.reads.fetch_add(requests, Ordering::Relaxed);
        }
    }

    /// Counts time one of its threads' requests waited for a device's
    /// room, and time it was held to the domain's rates.
    pub fn count_disk_wait(&self, wait_ns: u64, throttled_ns: u64) {
        if wait_ns > 0 {
            self.disk.wait_ns.fetch_add(wait_ns, Ordering::Relaxed);
        }
        if throttled_ns > 0 {
            self.disk.throttled_ns.fetch_add(throttled_ns, Ordering::Relaxed);
        }
    }

    /// What it and the domains below it moved to and from disks: its own
    /// counts and those of each domain below it in the table. One that
    /// has left the table but not yet gone (threads in it hold it) is not
    /// counted until it goes.
    pub fn disk_use(&self) -> DiskUse {
        let mut used = self.disk.read();
        for_each(|domain| {
            if !core::ptr::eq(Arc::as_ptr(domain), self) && domain.is_within(self) {
                used.add(&domain.disk.read());
            }
        });
        used
    }

    /// How many times the out-of-memory policy has stopped it.
    pub fn oom_stops(&self) -> u32 {
        self.oom_stops.load(Ordering::Relaxed)
    }

    /// Whether it is being stopped by the out-of-memory policy.
    pub fn is_stopping(&self) -> bool {
        self.state.load(Ordering::Acquire) & STOPPING != 0
    }

    /// Whether it is `other` or below it.
    pub fn is_within(&self, other: &Domain) -> bool {
        let mut at = Some(self);
        while let Some(domain) = at {
            if core::ptr::eq(domain, other) {
                return true;
            }
            at = domain.parent.as_deref();
        }
        false
    }

    /// Charges `pages` to it and every domain above it, unless that would
    /// pass one of their limits; then nothing is charged.
    pub fn charge(&self, pages: u64) -> Result<(), DomainError> {
        let mut at = Some(self);
        while let Some(domain) = at {
            let before = domain.usage.fetch_add(pages, Ordering::AcqRel);
            if before.saturating_add(pages) > domain.limit.load(Ordering::Relaxed) {
                domain.usage.fetch_sub(pages, Ordering::AcqRel);
                // Undo the domains below this one.
                let mut undo = Some(self);
                while let Some(below) = undo {
                    if core::ptr::eq(below, domain) {
                        break;
                    }
                    below.usage.fetch_sub(pages, Ordering::AcqRel);
                    undo = below.parent.as_deref();
                }
                self.refused.fetch_add(1, Ordering::Relaxed);
                return Err(DomainError::OverLimit);
            }
            domain.peak.fetch_max(before + pages, Ordering::Relaxed);
            at = domain.parent.as_deref();
        }
        Ok(())
    }

    /// Gives back `pages` charged with [`Domain::charge`].
    pub fn uncharge(&self, pages: u64) {
        let mut at = Some(self);
        while let Some(domain) = at {
            domain.usage.fetch_sub(pages, Ordering::AcqRel);
            at = domain.parent.as_deref();
        }
    }

    /// A thread joins it: refused while it is being stopped, or when it
    /// or a domain above it has as many threads as its limit allows, and
    /// then nothing is counted.
    pub(crate) fn thread_joined(&self) -> Result<(), JoinError> {
        self.count_within()?;
        if self.update(|state| (state & STOPPING == 0).then_some(state + 1)).is_none() {
            self.uncount_within(None);
            return Err(JoinError::Stopping);
        }
        Ok(())
    }

    /// Counts one more thread within it and every domain above it, unless
    /// that would pass one of their thread limits, as [`Domain::charge`]
    /// does for pages; then nothing is counted.
    fn count_within(&self) -> Result<(), JoinError> {
        let mut at = Some(self);
        while let Some(domain) = at {
            let before = domain.within.fetch_add(1, Ordering::AcqRel);
            if before >= domain.thread_limit.load(Ordering::Relaxed) {
                domain.within.fetch_sub(1, Ordering::AcqRel);
                self.uncount_within(Some(domain));
                return Err(JoinError::TooManyThreads);
            }
            at = domain.parent.as_deref();
        }
        Ok(())
    }

    /// Counts one thread fewer within it and the domains above it, up to
    /// `until` and not it, or all of them.
    fn uncount_within(&self, until: Option<&Domain>) {
        let mut at = Some(self);
        while let Some(domain) = at {
            if until.is_some_and(|until| core::ptr::eq(domain, until)) {
                break;
            }
            domain.within.fetch_sub(1, Ordering::AcqRel);
            at = domain.parent.as_deref();
        }
    }

    /// A thread in it exited (or was freed without running). Returns
    /// whether it was the domain's last. A stopped domain without a
    /// supervisor in the kernel takes new threads again once its last has
    /// gone: Spark, which made it, starts its service again (Phase 9).
    pub(crate) fn thread_left(&self) -> bool {
        let unsupervised = self.supervisor.is_none();
        let before = self
            .update(|state| {
                let left = state - 1;
                Some(if left & THREAD_COUNT == 0 && unsupervised { left & !STOPPING } else { left })
            })
            .unwrap_or(0);
        self.uncount_within(None);
        before & THREAD_COUNT == 1
    }

    /// Changes the state word with `change` (None: leave it), as one
    /// atomic step; returns the word as it was, if it changed.
    fn update(&self, change: impl FnMut(u64) -> Option<u64>) -> Option<u64> {
        self.state.try_update(Ordering::AcqRel, Ordering::Acquire, change).ok()
    }

    /// What to wait on (`sched::wait`) for its last thread to exit.
    pub fn key(&self) -> usize {
        core::ptr::from_ref::<Domain>(self) as usize
    }

    /// Marks it stopping (or not): from the out-of-memory policy, counted
    /// in [`Domain::oom_stops`], or from its holder's `stop`; and not, from
    /// its supervisor once it has restarted it. A domain without threads
    /// or a supervisor has nothing to wait for and is not marked.
    pub(crate) fn set_stopping(&self, stopping: bool, oom: bool) {
        if stopping && oom {
            self.oom_stops.fetch_add(1, Ordering::Relaxed);
        }
        let unsupervised = self.supervisor.is_none();
        self.update(|state| match stopping {
            true if unsupervised && state & THREAD_COUNT == 0 => None,
            true => Some(state | STOPPING),
            false => Some(state & !STOPPING),
        });
    }
}

/// Every domain, for the out-of-memory policy and the debugger.
static DOMAINS: SpinLock<Option<Vec<DomainRef, Kmalloc>>> = SpinLock::new(None);
/// The root domain.
static ROOT: Once<DomainRef> = Once::new();
static NEXT_ID: AtomicU32 = AtomicU32::new(0);

fn kmalloc() -> Kmalloc {
    HEAP.with(Flags::MAY_SLEEP)
}

/// Makes the root domain. Call once, before the scheduler starts.
pub fn init() {
    let mut list = Vec::new_in(kmalloc());
    let root = Arc::try_new_in(
        new_domain(Name::new("system"), None, 0, Settings::new(OomClass::System), None),
        kmalloc(),
    );
    match (list.try_reserve_exact(MAX_DOMAINS), root) {
        (Ok(()), Ok(root)) => {
            let _ = list.push_within_capacity(root.clone());
            *DOMAINS.lock_irq() = Some(list);
            let _ = ROOT.set(root);
        }
        _ => panic!("Domains: no memory for the domain table at boot"),
    }
}

fn new_domain(
    name: Name,
    parent: Option<DomainRef>,
    depth: u8,
    settings: Settings,
    object: Option<ObjectId>,
) -> Domain {
    Domain {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        name,
        parent,
        depth,
        class: settings.class,
        supervisor: settings.supervisor,
        object,
        weight: AtomicU32::new(settings.weight),
        limit: AtomicU64::new(settings.limit_pages.unwrap_or(u64::MAX)),
        usage: AtomicU64::new(0),
        peak: AtomicU64::new(0),
        refused: AtomicU64::new(0),
        state: AtomicU64::new(0),
        oom_stops: AtomicU32::new(0),
        within: AtomicU32::new(0),
        thread_limit: AtomicU32::new(settings.thread_limit.unwrap_or(u32::MAX)),
        disk: DiskCounts::default(),
        io_weight: AtomicU32::new(IO_WEIGHT_DEFAULT),
        io_rates: SpinLock::new([None; 4]),
    }
}

/// The root domain, `system`.
pub fn root() -> DomainRef {
    ROOT.get().expect("domains::init runs before anything asks for the root").clone()
}

/// Makes a domain below `parent`, named by capability object `object` if
/// a capability is made for it.
pub(crate) fn create(
    name: &str,
    parent: &DomainRef,
    settings: Settings,
    object: Option<ObjectId>,
) -> Result<DomainRef, DomainError> {
    if !(1..=DOMAIN_WEIGHT_MAX).contains(&settings.weight)
        || settings.class > parent.class
        || parent.depth >= MAX_DEPTH
    {
        return Err(DomainError::InvalidArgument);
    }
    let domain = Arc::try_new_in(
        new_domain(Name::new(name), Some(parent.clone()), parent.depth + 1, settings, object),
        kmalloc(),
    )
    .map_err(|_| DomainError::NoMemory)?;
    let mut list = DOMAINS.lock_irq();
    let list = list.as_mut().ok_or(DomainError::NoMemory)?;
    list.push_within_capacity(domain.clone()).map_err(|_| DomainError::NoMemory)?;
    Ok(domain)
}

/// The domain named by capability object `object`.
pub(crate) fn by_object(object: ObjectId) -> Option<DomainRef> {
    let list = DOMAINS.lock_irq();
    list.as_ref()?.iter().find(|d| d.object == Some(object)).cloned()
}

/// The capability store let go of `object`: the domain leaves the table.
/// It lives on while threads in it or domains below it hold it.
pub(crate) fn released(object: ObjectId) {
    let removed = {
        let mut list = DOMAINS.lock_irq();
        let Some(list) = list.as_mut() else { return };
        list.iter().position(|d| d.object == Some(object)).map(|index| list.swap_remove(index))
    };
    drop(removed);
}

/// Takes `domain`, made without a capability, off the table, so it goes
/// once the last thread and reference let go of it.
pub(crate) fn remove(domain: &DomainRef) {
    let removed = {
        let mut list = DOMAINS.lock_irq();
        let Some(list) = list.as_mut() else { return };
        list.iter().position(|d| DomainRef::ptr_eq(d, domain)).map(|index| list.swap_remove(index))
    };
    drop(removed);
}

/// Runs `f` on every domain in the table, with its lock held: `f` must not
/// take locks or allocate.
pub fn for_each(mut f: impl FnMut(&DomainRef)) {
    let list = DOMAINS.lock_irq();
    for domain in list.iter().flatten() {
        f(domain);
    }
}

/// Like [`for_each`], but gives up if another CPU holds the table: for
/// `kdb`.
pub fn try_for_each(mut f: impl FnMut(&DomainRef)) -> bool {
    let Some(list) = DOMAINS.try_lock_irq() else { return false };
    for domain in list.iter().flatten() {
        f(domain);
    }
    true
}

/// Writes the domain table, for `kdb`.
pub fn write_table(line: &mut dyn FnMut(fmt::Arguments<'_>)) {
    line(format_args!(
        "Domains (class, CPU weight, pages used / limit, peak, refused charges, threads, \
         out-of-memory stops):"
    ));
    let shown = try_for_each(|d| {
        let mut limit = radix_klog::TextBuf::<24>::new();
        let _ = match d.limit_pages() {
            u64::MAX => fmt::Write::write_str(&mut limit, "none"),
            pages => fmt::Write::write_fmt(&mut limit, format_args!("{pages}")),
        };
        let mut name = radix_klog::TextBuf::<32>::new();
        for _ in 0..d.depth() {
            name.push_str("  ");
        }
        name.push_str(d.name().as_str());
        line(format_args!(
            "  {:>3} {:<24} {:<8} {:>5} {:>8} / {:<8} {:>8} {:>6} {:>4} {:>3}{}",
            d.id(),
            name,
            d.class().name(),
            d.weight(),
            d.usage_pages(),
            limit,
            d.peak_pages(),
            d.refused(),
            d.threads(),
            d.oom_stops(),
            if d.is_stopping() { " stopping" } else { "" }
        ));
    });
    if !shown {
        line(format_args!("  another CPU holds the domain table"));
    }
}
