// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Timer files (ADR 0613, radix-process(2)'s `timer_file`): a clock and
//! a timer that radix-file(2)'s `timer_set` arms, whose 8-byte reads give
//! how many times it has expired since the last read, as Linux's
//! timerfd(2).
//!
//! A timer is set to expire once, or then every interval. Its expiries
//! are counted, not queued: a read takes the count and sets it to 0,
//! waiting while it is 0 unless the description is non-blocking. A poll
//! finds it readable while the count is not 0. Setting it again forgets
//! the count.
//!
//! The kernel thread `timer files` counts the expiries of the armed timers
//! ([`ARMED`]): it sleeps until the first is due, adds the expiries since
//! to each that has passed, wakes the readers and the pollers, and sleeps
//! again. It is not the thread of `posix::timer`, whose timers belong to a
//! process's POSIX state and end with it and send signals: a timer file
//! belongs to nothing but its descriptions, and delivers no signal. It
//! follows the same rules: setting a timer wakes it only if it would sleep
//! past the new expiry, and turning one off never does (ADR 0650). A read
//! counts the expiries that are due itself before it looks, and a poll and
//! `timer_get` count them on a copy, so what they find does not wait for
//! the thread, and only the thread and a read change the count.
//!
//! A timer on the realtime clock holds its expiry as a time of day, so
//! that setting the clock moves when it expires as it does on Linux. Each
//! look turns it into the kernel's clock, and [`clock_was_set`], called
//! by the methods that set the wall clock, has the thread look again and
//! cancels the timers armed with cancel_on_set: such a timer becomes
//! readable, and its next read fails with cancelled, once.
//!
//! When it misbehaves, the boot test `posix` runs `kernelfiles.c`.

use alloc::sync::Weak;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::file::TimerSetFlags;
use radix_abi::process::{ClockKind, PipeFlags};
use radix_hal::Timer;
use radix_klog::error;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::{Mutex, MutexGuard, SpinLock};
use radix_vfs::{Access, KArc, Node, NodeRef, Out, Stat, mem, next_id};

use super::own_file;
use crate::Arch;
use crate::sched::{self, Options, Policy, wait, wait::WaitCount};
use crate::user::UserRef;
use crate::user::posix::poll::{self, Polled};
use crate::user::posix::signal::{Restart, interrupted};

/// A timer file's node.
pub struct TimerFile {
    id: u64,
    /// The clock it counts: realtime, monotonic or boottime, the last two
    /// being the kernel's clock.
    clock: ClockKind,
    state: SpinLock<State>,
    /// Whether a read would find an expiry or a cancellation, kept beside
    /// `state` for the waits, whose conditions read no lock.
    ready: AtomicBool,
    /// The threads waiting to read.
    readers: WaitCount,
    /// Its pollers.
    polled: Polled,
}

/// What a timer file is set to, and what has happened to it.
#[derive(Clone, Copy)]
struct State {
    /// When it expires next: a time on the kernel's clock, or for a
    /// realtime timer a time of day in nanoseconds since 1970. `None`
    /// when it is not set, or was set to expire once and has.
    expires: Option<u64>,
    /// How long after expiring it expires again (0: it does not).
    interval_ns: u64,
    /// How many times it has expired since it was last read or set.
    ticks: u64,
    /// Whether setting the wall clock cancels it: it was set absolute on
    /// the realtime clock with cancel_on_set.
    cancel_on_set: bool,
    /// The wall clock was set since it was last read or set.
    cancelled: bool,
}

impl State {
    /// Counts the expiries that are due, `mono` being the kernel's clock
    /// and `wall` the time of day: returns whether there were any. An
    /// interval makes the next expiry after now, as `posix::timer`'s
    /// intervals do; expiries missed meanwhile count once each.
    fn advance(&mut self, clock: ClockKind, mono: u64, wall: u64) -> bool {
        let now = if clock == ClockKind::Realtime { wall } else { mono };
        let Some(expires) = self.expires.filter(|&expires| now >= expires) else {
            return false;
        };
        match (now - expires).checked_div(self.interval_ns) {
            None => {
                self.ticks = self.ticks.saturating_add(1);
                self.expires = None;
            }
            Some(missed) => {
                self.ticks = self.ticks.saturating_add(missed.saturating_add(1));
                let next = missed.saturating_add(1).saturating_mul(self.interval_ns);
                self.expires = Some(expires.saturating_add(next));
            }
        }
        true
    }

    /// Whether a read would find something.
    fn readable(&self) -> bool {
        self.ticks > 0 || self.cancelled
    }
}

impl Node for TimerFile {
    fn stat(&self) -> Stat {
        own_file::stat(self.id)
    }

    fn access(&self) -> Access {
        own_file::access()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The timer file `node` is, if it is one.
pub fn timer_of(node: &NodeRef) -> Option<&TimerFile> {
    radix_vfs::downcast::<TimerFile>(&**node)
}

/// Makes a timer file on `clock` for `user`'s process, in slot `to` with
/// `flags` (radix-process(2)'s `timer_file`).
///
/// # Errors
///
/// invalid_argument for a clock that is not realtime, monotonic or
/// boottime, no_memory, and the slot's status if the file cannot go
/// there.
pub fn make(user: &UserRef, to: u32, clock: ClockKind, flags: PipeFlags) -> Result<(), Status> {
    if !matches!(clock, ClockKind::Realtime | ClockKind::Monotonic | ClockKind::Boottime) {
        return Err(Kernel::InvalidArgument.status(1));
    }
    let file = TimerFile {
        id: next_id(),
        clock,
        state: SpinLock::new(State {
            expires: None,
            interval_ns: 0,
            ticks: 0,
            cancel_on_set: false,
            cancelled: false,
        }),
        ready: AtomicBool::new(false),
        readers: WaitCount::new(),
        polled: Polled::new(),
    };
    let node: NodeRef = mem::arc(file).map_err(|_| Kernel::NoMemory.status(0))?;
    own_file::install(user, node, to, flags)
}

/// The kernel's clock and the time of day, read together.
fn clocks() -> (u64, u64) {
    let wall = <Arch as Timer>::wall_clock_ns().unwrap_or(0);
    (sched::now_ns(), wall)
}

impl TimerFile {
    fn key(&self) -> usize {
        core::ptr::from_ref::<TimerFile>(self) as usize
    }

    /// What its pollers count themselves in.
    pub fn polled(&self) -> &Polled {
        &self.polled
    }

    /// The first time, on the kernel's clock, that the thread must look at
    /// a timer that expires at `expires` on this file's clock, `mono` and
    /// `wall` being the two clocks now.
    fn look_by(&self, expires: u64, mono: u64, wall: u64) -> u64 {
        if self.clock == ClockKind::Realtime {
            mono.saturating_add(expires.saturating_sub(wall))
        } else {
            expires
        }
    }

    /// Something was counted, or cancelled: readers and pollers look again.
    fn changed(&self) {
        self.readers.wake_all(self.key());
        // `wake_all` took the full barrier that `Polled::changed` would.
        self.polled.changed_by_rmw();
    }

    /// What `poll` finds: readable while a read would find an expiry or a
    /// cancellation. It counts what is due on a copy of the state: an
    /// expiry it counted in the state itself would not wake the readers
    /// waiting for one, as the thread's count does, and a poll cannot wake
    /// them, as a queue may look at the file with its own lock held.
    pub fn events(&self) -> u16 {
        let (mono, wall) = clocks();
        let mut state = *self.state.lock();
        state.advance(self.clock, mono, wall);
        if state.readable() { poll::READABLE } else { 0 }
    }

    /// Counts what is due for the thread, which is to look at it: returns
    /// when it must look again (`None`: not before the timer is set), and
    /// whether it must look at all after this look, a timer with nothing
    /// to expire or cancel being left out of the list.
    fn expire(&self, mono: u64, wall: u64) -> (Option<u64>, bool) {
        let (ticked, next, keep) = {
            let mut state = self.state.lock();
            let ticked = state.advance(self.clock, mono, wall);
            if ticked {
                self.ready.store(true, Ordering::Release);
            }
            let next = state.expires.map(|expires| self.look_by(expires, mono, wall));
            (ticked, next, state.expires.is_some() || state.cancel_on_set)
        };
        if ticked {
            self.changed();
        }
        (next, keep)
    }

    /// The wall clock was set: a timer armed with cancel_on_set becomes
    /// readable, and its next read fails with cancelled.
    fn cancel(&self) {
        {
            let mut state = self.state.lock();
            if !state.cancel_on_set {
                return;
            }
            state.cancelled = true;
            self.ready.store(true, Ordering::Release);
        }
        self.changed();
    }

    /// How long until it expires (0: it will not) and its interval, as of
    /// the clocks `mono` and `wall`, which the caller has locked `state`
    /// after reading. A timer due now is a nanosecond off, never 0, so
    /// that setting it again with what was read does not turn it off.
    fn left(&self, state: &State, mono: u64, wall: u64) -> (u64, u64) {
        let now = if self.clock == ClockKind::Realtime { wall } else { mono };
        let value = state.expires.map_or(0, |expires| expires.saturating_sub(now).max(1));
        (value, state.interval_ns)
    }

    /// How long until it expires (0: it will not) and its interval, as
    /// radix-file(2)'s `timer_get` gives them: counted on a copy of the
    /// state, as [`TimerFile::events`] counts.
    pub fn get(&self) -> (u64, u64) {
        let (mono, wall) = clocks();
        let mut state = *self.state.lock();
        state.advance(self.clock, mono, wall);
        self.left(&state, mono, wall)
    }

    /// Takes what a read takes: the expiries counted, or the cancellation,
    /// which is not counted but reported once.
    fn take(&self) -> Taken {
        let (mono, wall) = clocks();
        let mut state = self.state.lock();
        state.advance(self.clock, mono, wall);
        let taken = if state.cancelled {
            state.cancelled = false;
            state.ticks = 0;
            Taken::Cancelled
        } else if state.ticks > 0 {
            Taken::Ticks(core::mem::take(&mut state.ticks))
        } else {
            Taken::Nothing
        };
        self.ready.store(state.readable(), Ordering::Release);
        taken
    }

    /// Puts back the `ticks` a read took, which it could not give to the
    /// program, and has the readers and pollers look again.
    fn give_back(&self, ticks: u64) {
        {
            let mut state = self.state.lock();
            state.ticks = state.ticks.saturating_add(ticks);
            self.ready.store(true, Ordering::Release);
        }
        self.changed();
    }

    /// Reads how many times it has expired into `out`, as 8 bytes,
    /// waiting while that is 0 unless `non_blocking`.
    ///
    /// # Errors
    ///
    /// invalid_argument for an `out` of fewer than 8 bytes, cancelled
    /// once after the wall clock was set under a timer armed with
    /// cancel_on_set, would_block for no expiry when `non_blocking`, and
    /// interrupted (restarting) for a wait a signal cut short.
    pub fn read(&self, out: &mut dyn Out, non_blocking: bool) -> Result<usize, Status> {
        if out.len() < 8 {
            return Err(Kernel::InvalidArgument.status(0));
        }
        loop {
            match self.take() {
                Taken::Ticks(ticks) => {
                    if let Err(err) = out.put(0, &ticks.to_le_bytes()) {
                        self.give_back(ticks);
                        return Err(super::status(err));
                    }
                    return Ok(8);
                }
                Taken::Cancelled => return Err(Kernel::Cancelled.status(0)),
                Taken::Nothing => {}
            }
            if non_blocking {
                return Err(Kernel::WouldBlock.status(0));
            }
            self.readers.wait_until(self.key(), "timer file", || {
                self.ready.load(Ordering::Acquire) || sched::interrupted()
            });
            if sched::interrupted() && !self.ready.load(Ordering::Acquire) {
                return Err(interrupted(Restart::Always));
            }
        }
    }
}

/// What a read found.
enum Taken {
    /// It had expired this many times.
    Ticks(u64),
    /// The wall clock was set under it.
    Cancelled,
    /// Nothing yet.
    Nothing,
}

/// Arms the timer file of `node` (radix-file(2)'s `timer_set`): to expire
/// `value_ns` from now, or with absolute at `value_ns` on its clock, then
/// every `interval_ns`; `value_ns` 0 disarms it. Returns what was left of
/// the old setting and its interval.
///
/// # Errors
///
/// invalid_argument if `node` is no timer file, and no_memory if the timer
/// cannot be listed or its thread started.
pub fn set(
    node: &NodeRef,
    flags: TimerSetFlags,
    value_ns: u64,
    interval_ns: u64,
) -> Result<(u64, u64), Status> {
    let file = timer_of(node).ok_or(Kernel::InvalidArgument.status(0))?;
    let absolute = flags.contains(TimerSetFlags::ABSOLUTE);
    let cancel_on_set = absolute
        && flags.contains(TimerSetFlags::CANCEL_ON_SET)
        && file.clock == ClockKind::Realtime;
    // Listed before it is set, with the list held until it is: the thread
    // takes a timer with nothing to expire or cancel out of the list
    // under the same lock ([`unregister`]), so one set meanwhile would
    // otherwise never be looked at.
    let armed = if value_ns != 0 || cancel_on_set {
        start()?;
        Some(register(node)?)
    } else {
        None
    };
    let (mono, wall) = clocks();
    let (old, look_by) = {
        let mut state = file.state.lock();
        state.advance(file.clock, mono, wall);
        let old = file.left(&state, mono, wall);
        let now = if file.clock == ClockKind::Realtime { wall } else { mono };
        state.expires = match (value_ns, absolute) {
            (0, _) => None,
            (_, true) => Some(value_ns),
            (_, false) => Some(now.saturating_add(value_ns)),
        };
        state.interval_ns = interval_ns;
        state.ticks = 0;
        state.cancelled = false;
        state.cancel_on_set = cancel_on_set;
        file.ready.store(false, Ordering::Release);
        let look_by = state.expires.map_or(u64::MAX, |expires| file.look_by(expires, mono, wall));
        (old, look_by)
    };
    drop(armed);
    changed_by(look_by);
    Ok(old)
}

/// The timer files that are armed, or armed to be cancelled: weakly, so that a
/// closed one leaves the list when the thread next looks at it.
type Armed = Vec<Weak<dyn Node, Kmalloc>, Kmalloc>;

/// The timer files with a timer set or armed with cancel_on_set, by weak
/// references, so that one every description of which is closed drops out
/// of the list. Its lock is taken before a timer file's state, and held
/// while the thread counts what is due.
static ARMED: Mutex<Option<Armed>> = Mutex::new(None);

/// Moves whenever a timer is set, so that the thread looks again.
static CHANGES: AtomicU64 = AtomicU64::new(0);

/// When the thread looks at the timers next, on the kernel's clock, unless
/// it is woken (`u64::MAX`: only when woken), kept as `posix::timer` keeps
/// its own: the thread writes it after each look, before its wait reads
/// [`CHANGES`], and a setter moves `CHANGES`, then reads it, each in the
/// single order of sequentially consistent operations, so that either the
/// wait sees the change or the setter sees the time the thread sleeps
/// until. It starts at 0: the thread looks as soon as it runs.
static NEXT_LOOK: AtomicU64 = AtomicU64::new(0);

/// Whether the thread has been started.
static STARTED: AtomicBool = AtomicBool::new(false);

fn changes_key() -> usize {
    &raw const CHANGES as usize
}

/// Has the thread look at the timers again by `look_by`, on the kernel's
/// clock (`u64::MAX`: whenever it next looks, for a timer turned off): it
/// is woken only if it would sleep past then.
fn changed_by(look_by: u64) {
    CHANGES.fetch_add(1, Ordering::SeqCst);
    if look_by < NEXT_LOOK.load(Ordering::SeqCst) {
        wait::wake_all(changes_key());
    }
}

/// The wall clock was set (radix-clock(2)'s `set`): the timers armed with
/// cancel_on_set are cancelled, and the thread looks at the realtime ones
/// again, whose expiries are times of day.
pub fn clock_was_set() {
    {
        let armed = ARMED.lock();
        let timers = armed.iter().flatten().filter_map(Weak::upgrade);
        for node in timers {
            if let Some(file) = radix_vfs::downcast::<TimerFile>(&*node) {
                file.cancel();
            }
        }
    }
    changed_by(0);
}

/// The list of [`ARMED`], locked.
type ArmedGuard = MutexGuard<'static, Option<Vec<Weak<dyn Node, Kmalloc>, Kmalloc>>>;

/// Puts the timer file `node` in [`ARMED`], unless it is there, and
/// returns the list still locked: the caller sets the timer before letting
/// go of it.
fn register(node: &NodeRef) -> Result<ArmedGuard, Status> {
    let mut armed = ARMED.lock();
    let list = armed.get_or_insert_with(|| Vec::new_in(HEAP.with(Flags::MAY_SLEEP)));
    let at = radix_vfs::address(node);
    if list.iter().any(|weak| weak.as_ptr().cast::<u8>() as usize == at) {
        return Ok(armed);
    }
    list.retain(|weak| weak.strong_count() > 0);
    list.try_reserve(1).map_err(|_| Kernel::NoMemory.status(0))?;
    // Room was reserved above.
    let _ = list.push_within_capacity(KArc::downgrade(node));
    Ok(armed)
}

/// Starts the thread, once.
fn start() -> Result<(), Status> {
    // Read first: every `timer_set` comes here.
    if STARTED.load(Ordering::Acquire) || STARTED.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    match sched::spawn("timer files", run, 0, Options::new().policy(Policy::Fair { nice: -5 })) {
        Ok(_) => Ok(()),
        Err(err) => {
            STARTED.store(false, Ordering::Release);
            error!("User: could not start the timer files' thread: {err}");
            Err(Kernel::NoMemory.status(0))
        }
    }
}

/// The thread.
fn run(_: usize) {
    loop {
        let seen = CHANGES.load(Ordering::SeqCst);
        let deadline = expire_due();
        // Before the wait reads CHANGES ([`NEXT_LOOK`]).
        NEXT_LOOK.store(deadline.unwrap_or(u64::MAX), Ordering::SeqCst);
        let changed = || CHANGES.load(Ordering::SeqCst) != seen;
        match deadline {
            Some(deadline) => {
                wait::wait_until_or_timeout(changes_key(), "timer files", deadline, changed);
            }
            None => wait::wait_until(changes_key(), "timer files", changed),
        }
    }
}

/// Counts the expiries due in every listed timer file, wakes the readers
/// of those that had any, takes out of the list the files that are gone or
/// have nothing to expire, and returns when the thread must look next
/// (`None`: when a timer is set). The list's lock is held throughout,
/// which makes a setter wait the time it takes: a look at each file's
/// state, and a wake for those that expired.
fn expire_due() -> Option<u64> {
    let mut armed = ARMED.lock();
    let list = armed.as_mut()?;
    let (mono, wall) = clocks();
    let mut next = u64::MAX;
    list.retain(|weak| {
        let Some(node) = weak.upgrade() else { return false };
        let Some(file) = radix_vfs::downcast::<TimerFile>(&*node) else { return false };
        let (look_by, keep) = file.expire(mono, wall);
        next = next.min(look_by.unwrap_or(u64::MAX));
        keep
    });
    (next != u64::MAX).then_some(next)
}
