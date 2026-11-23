// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Programs in user mode (Phases 5 and 6): their processes and threads,
//! the twelve system calls, and the kernel objects they work with.
//!
//! A program runs as a process (`process`): an address space, a
//! capability space and threads, which end together. A program's thread
//! is a scheduler thread with a [`UserThread`] attached: its process, the
//! capability space it names capabilities in, the address space it runs
//! in, its IPC state and its signals. The architecture takes it into user
//! mode once ([`UserMode::enter_user`]) and back into the kernel through
//! system calls (`syscall`), interrupts and faults. A fault in user mode
//! that no page the kernel makes on demand resolves raises a signal
//! (Phase 8), which ends the process unless a handler takes it; nothing a
//! program does stops the kernel.
//!
//! Kernel objects are kept in the capability store, each in the record of
//! the name the store gives it ([`objects`]), so a capability lookup leads
//! to the object itself. The store decides what a program may do; the objects carry it
//! out:
//!
//! - `ipc`: endpoints, synchronous messages of up to eight words and four
//!   capabilities, with a fast path for register-only messages;
//! - `notify`: notifications, words of bits that are signalled and waited
//!   for;
//! - `memory`: address spaces and frames, mapped and unmapped;
//! - `ring`: rings shared with programs (radix-ring);
//! - `process`: processes, and the kernel memory their objects take,
//!   charged to resource domains (`account`);
//! - `files`: files and directories (Phase 7, `crate::fs`);
//! - `posix`: the POSIX side of processes (Phase 8): the process tree,
//!   signals, `fork`, `exec`, timers and `poll`;
//! - `methods`: the methods called on capabilities with `cap_call`.
//!
//! The kernel itself starts one program, Spark, from the boot archive
//! (`init`), and the test programs of the `usertests` boot module
//! (`launch`) for the `user`, `ipc`, `fuzz` and `bench` self-tests
//! (`usertest`); Spark and its runtime start every other program.
//! `docs/design/ipc.md` and `docs/design/processes.md` describe the
//! design.
//!
//! Lock order: the capability store (an object's record last), a process's
//! state, a frame's state, an address space's regions; an endpoint, then
//! a thread's IPC state; a ring's queues. None is held across a block.

pub(crate) mod account;
mod clock;
pub mod crash;
pub mod device;
mod domain;
pub(crate) mod files;
mod firewall;
pub mod init;
mod ipc;
pub mod launch;
mod linux;
mod loader;
pub(crate) mod memory;
mod methods;
pub mod names;
pub mod network;
mod notices;
mod notify;
pub mod objects;
mod platform;
pub(crate) mod posix;
pub mod power;
pub mod process;
pub mod queue;
pub mod random;
mod ring;
mod settings;
mod syscall;
mod vdso;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

pub use radix_abi::process::{EXIT_FAULT, EXIT_STOPPED};

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::startup;
use radix_cap::{CapError, SpaceId};
use radix_hal::{Cpu, CpuLocal, InterruptController, Ipi, UserFault, UserFaultKind, UserMode};
use radix_klog::{info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::{Mutex, SpinLock};

use crate::Arch;
use crate::sched::{self, ThreadRef, wait};
use memory::{Fault, SpaceObject};
use posix::ThreadSignals;
use posix::priority::Scheduling;
use process::{Ending, ProcessRef, Start};

/// A program thread's side, shared.
pub type UserRef = Arc<UserThread, Kmalloc>;

/// What a thread of a program has besides its scheduler side.
pub struct UserThread {
    /// The capability space its slot numbers name.
    cspace: SpaceId,
    /// The address space it runs in.
    space: memory::SpaceRef,
    /// Where it starts in user mode.
    start: Start,
    /// Set once whoever started it has finished setting it up.
    ready: AtomicBool,
    /// Its IPC state.
    ipc: SpinLock<ipc::IpcState>,
    /// The caller it owes a reply, after receiving a call.
    reply: SpinLock<Option<ipc::Caller>>,
    /// Its receive window: first slot (high half) and count (low half).
    window: AtomicU64,
    /// The process it is a thread of.
    pub(crate) process: ProcessRef,
    /// Its signal mask, the signals sent to it alone, and the rest of its
    /// signal state (Phase 8).
    pub(crate) signals: SpinLock<ThreadSignals>,
    /// Set by the `futex_wake` that takes it off a futex's waiters.
    pub(crate) futex_woken: AtomicBool,
    /// The list of futex waiters it is in while it waits, which a requeue
    /// may change (`memory::futex`).
    pub(crate) futex_bucket: AtomicUsize,
    /// Set by a change to what it polls while it waits in `poll`, which
    /// then looks again; cleared before each look (`posix::poll`).
    pub(crate) poll_changed: AtomicBool,
    /// Its scheduling as POSIX names it, which its scheduler side follows
    /// (Phase 8).
    pub(crate) scheduling: Mutex<Scheduling>,
    /// Bytes charged to its process's account that the files it opens are
    /// charged from (ADR 0405), given back when it ends.
    pub(crate) stock: account::Stock,
    /// Its Linux words, for a thread of a Linux program (ADR 0611).
    pub(crate) linux: linux::LinuxThread,
}

impl Drop for UserThread {
    fn drop(&mut self) {
        // A thread that never ran, or one whose stock its end missed.
        self.stock.drain(self.process.account());
    }
}

impl UserThread {
    /// The address space.
    pub fn space(&self) -> &SpaceObject {
        &self.space
    }

    /// The receive window: first slot and count.
    fn window(&self) -> (u32, u32) {
        let word = self.window.load(Ordering::Relaxed);
        ((word >> 32) as u32, word as u32)
    }
}

/// A system call's result: its result words, or an error status.
type Result<T> = core::result::Result<T, Status>;

/// An error status with no detail.
fn error(kind: Kernel) -> Status {
    kind.status(0)
}

/// The status a capability error becomes. Missing rights put the rights
/// that were missing in the detail.
fn cap_error(err: CapError) -> Status {
    match err {
        CapError::NoSuchSpace => error(Kernel::Gone),
        CapError::NoSuchSlot => error(Kernel::NoSuchSlot),
        CapError::Empty => error(Kernel::EmptySlot),
        CapError::Occupied => error(Kernel::Occupied),
        CapError::WrongKind { found, .. } => Kernel::WrongKind.status(found as u32),
        CapError::MissingRights(rights) => Kernel::MissingRights.status(u32::from(rights.bits())),
        CapError::Sealed => error(Kernel::Sealed),
        CapError::NotSealed => error(Kernel::InvalidArgument),
        CapError::WrongKey => error(Kernel::WrongKey),
        CapError::NotBadgeable(_) | CapError::AlreadyBadged | CapError::InvalidBadge => {
            error(Kernel::BadBadge)
        }
        CapError::NotARange | CapError::OutOfRange => error(Kernel::OutOfRange),
        CapError::TooDeep => error(Kernel::TooDeep),
        CapError::Revoking => error(Kernel::Revoking),
        CapError::Exhausted => error(Kernel::Exhausted),
        CapError::NoMemory => error(Kernel::NoMemory),
        CapError::InvalidArgument => error(Kernel::InvalidArgument),
    }
}

/// The running thread and its program side, if it is a program's thread.
fn current() -> Option<(ThreadRef, UserRef)> {
    let thread = sched::current()?;
    let user = thread.user()?.clone();
    Some((thread, user))
}

/// Whether `user` is the running thread's program side: only that thread
/// charges from its stock or gives back to it (`account::Stock`).
fn runs(user: &UserThread) -> bool {
    sched::current()
        .is_some_and(|thread| thread.user().is_some_and(|me| core::ptr::eq(Arc::as_ptr(me), user)))
}

/// Registers the fault handlers, and reports user mode. Called once at
/// boot. The system call and return handlers the architecture calls by
/// name ([`radix_syscall`], [`radix_user_return`]).
pub fn init() {
    vdso::init();
    Arch::set_user_fault_handler(on_fault);
    Arch::set_user_page_handler(on_user_page);
    Arch::describe_user_mode(&mut |args| info!("{args}"));
    // The ABI numbers object kinds as the store does.
    for kind in radix_cap::ObjectKind::ALL {
        let abi = radix_abi::capability::ObjectKind::from_raw(kind as u8);
        assert_eq!(abi.map(|abi| abi as u8), Some(kind as u8), "the ABI lacks the {kind} kind");
    }
    assert!(radix_abi::capability::ObjectKind::from_raw(radix_cap::KINDS as u8).is_none());
}

/// Where a program's thread starts: waits until whoever started it is
/// done with it, then leaves for user mode.
fn thread_main(_: usize) {
    let Some((thread, user)) = current() else { return };
    let key = Arc::as_ptr(&user) as usize;
    wait::wait_until(key, "starting", || {
        user.ready.load(Ordering::Acquire) || thread.should_stop()
    });
    // A Linux thread's CLONE_CHILD_SETTID word (ADR 0611), written while
    // a fault on it can still be taken.
    linux::starting(&thread, &user);
    // The last look before user mode is made with interrupts masked, and
    // they stay masked until the thread is there: a stop or a signal sent
    // after the look interrupts this CPU in user mode (`sched::interrupt`),
    // where the way back takes it, as it does after a system call.
    Arch::disable_interrupts();
    if thread.should_stop() {
        Arch::enable_interrupts();
        note_domain_stop(&thread, &user);
        drop(thread);
        exit_current(user, EXIT_STOPPED, Ending::Stop);
    }
    // A thread that has not been in user mode has no frame for a handler
    // yet: a signal sent to it before the look is taken on its first way
    // back, which an interrupt of this CPU to itself brings before its
    // first instruction, as Linux takes a new thread's signals.
    if thread.is_signalled() {
        Arch::send_ipi(Arch::current_cpu(), Ipi::Reschedule);
    }
    let start = user.start;
    // Nothing may stay on this stack: user mode abandons it.
    drop(thread);
    drop(user);
    match start {
        Start::Fresh([entry, stack, arg]) => Arch::enter_user(entry, stack, arg),
        Start::Registers(regs) => Arch::enter_user_registers(&regs),
    }
}

/// Lets a thread that waits to start go: it was set up, or is to stop.
fn set_ready(user: &UserThread) {
    user.ready.store(true, Ordering::Release);
    wait::wake_all(core::ptr::from_ref::<UserThread>(user) as usize);
}

/// Ends the running program thread with exit status `status`, which
/// ended `how`; its process hears of it (`ProcessObject::thread_exited`),
/// and may end with it.
fn exit_current(user: UserRef, status: u64, how: Ending) -> ! {
    end_thread(user, status, how, None)
}

/// Ends the running program thread with exit status `status`, as
/// radix-process(2)'s `exit_thread_waking` does: once its process no
/// longer has it, the word at `word` becomes 1 and its waiters wake.
fn exit_waking(user: UserRef, status: u64, word: u64) -> ! {
    end_thread(user, status, Ending::Exit, Some((word, 1)))
}

/// Ends the running thread of a Linux program as Linux's `exit` does for
/// a thread with a clear-child-tid word (ADR 0611): once its process no
/// longer has it, the word at `word` becomes 0 and its waiters wake.
fn exit_clearing(user: UserRef, status: u64, word: u64) -> ! {
    end_thread(user, status, Ending::Exit, Some((word, 0)))
}

/// [`exit_current`], and then, for [`exit_waking`] and [`exit_clearing`],
/// the joiner's word and what it becomes.
fn end_thread(user: UserRef, status: u64, how: Ending, word: Option<(u64, u32)>) -> ! {
    // The robust locks a Linux thread holds are marked as its owner's
    // (ADR 0617) before its joiner hears of its end, as on Linux.
    if let Some(me) = sched::current() {
        linux::ending(&me, &user);
    }
    ipc::exiting(&user);
    // What its stock holds goes back now, not when the last reference to
    // its side goes.
    user.stock.drain(user.process.account());
    let process = user.process.clone();
    // The thread's own space: the process lets go of its hold on it when
    // it ends, which this thread's leaving may make it do.
    let joiner = word.map(|(word, value)| (word, value, user.space.clone()));
    drop(user);
    if let Some(me) = sched::current() {
        process.thread_exited(&me, status, how);
    }
    drop(process);
    if let Some((word, value, space)) = joiner {
        wake_joiner(&space, word, value);
        // Dropped here: `sched::exit` does not return.
        drop(space);
    }
    sched::exit()
}

/// Sets the 32-bit word at `word` in the running thread's memory to
/// `value` and wakes every thread waiting on it. The word Dukovo's C
/// library's join waits on starts at 0 and becomes 1, so only its first
/// byte changes, and a reader sees 0 or 1 however the copy goes; a Linux
/// C library's holds the thread's number and becomes 0, and a reader that
/// sees part of the change waits on a value the word no longer has, which
/// returns at once. A word out of line with 4 bytes, or one the thread
/// cannot write, is left alone, and wakes nobody.
fn wake_joiner(space: &SpaceObject, word: u64, value: u32) {
    if word.is_multiple_of(4) && Arch::copy_to_user(word, &value.to_le_bytes()) {
        // An aligned word cannot fail.
        let _ = memory::futex::wake(space, word, u32::MAX, radix_abi::address_space::FUTEX_ALL);
    }
}

/// A fault in user mode: a page the kernel makes on demand (anonymous
/// memory, a copy of a page shared since `fork`), or else a signal for the
/// thread (`posix::signal::fault`), or the end of its process.
fn on_fault(fault: &UserFault) {
    if let UserFaultKind::Memory(_) = fault.kind {
        // The banks of this CPU's machine check, read before the thread can
        // move to another CPU: logged, and their pages retired.
        crate::mce::read();
    }
    Arch::enable_interrupts();
    let Some((thread, user)) = current() else {
        panic!("User mode: {fault} in a thread that is not a program's");
    };
    let mut bus = None;
    if let UserFaultKind::Memory(page) = fault.kind {
        // Its page leaves the space, so that the program's handler, if it
        // goes on, faults on it rather than meeting it again (ADR 0168).
        let at = page.and_then(|phys| user.space.retire_frame(phys));
        bus = Some((radix_posix::signal::BUS_MCEERR_AR, at.unwrap_or(0)));
    }
    if let UserFaultKind::Page { address, write, execute, .. } = fault.kind {
        match user.space.resolve(address, write, execute) {
            Fault::Resolved => return,
            Fault::Refused => {}
            Fault::Io => bus = Some((radix_posix::signal::BUS_ADRERR, address)),
            Fault::Poisoned => bus = Some((radix_posix::signal::BUS_MCEERR_AR, address)),
            Fault::NoMemory | Fault::Short => {
                warn!(
                    "User: thread {} ({}) ended: no memory for a page at {address:#x}",
                    thread.id(),
                    thread.name()
                );
                user.process.posix.lock().ended_by.get_or_insert(radix_abi::signal::SIGKILL);
                drop(thread);
                exit_current(user, EXIT_FAULT, Ending::Fault);
            }
        }
    }
    if posix::signal::fault(&thread, &user, fault, bus) {
        // Its handler runs, from the way back to user mode.
        return;
    }
    drop(thread);
    exit_current(user, EXIT_FAULT, Ending::Fault)
}

/// A page fault on user memory in one of the kernel's own copies: makes
/// the page if the space makes it on demand.
fn on_user_page(address: u64, write: bool) -> bool {
    let Some(thread) = sched::running() else { return false };
    let Some(user) = thread.user() else { return false };
    user.space.resolve(address, write, false) == Fault::Resolved
}

/// Copies `src` to the running program's memory at `dst` as
/// `Arch::copy_to_user` does, but without waiting for a page that is not
/// there yet: with preemption off, a fault on the copy ends it rather than
/// making the page (`on_user_page`), which may read a file. For a copy
/// under a lock that reading a file takes, a file's cache; the caller
/// then lets go of the lock and calls [`fault_in`].
pub(crate) fn copy_to_user_now(dst: u64, src: &[u8]) -> bool {
    Arch::preempt_disable();
    let copied = Arch::copy_to_user(dst, src);
    sched::preempt_enable();
    copied
}

/// [`copy_to_user_now`] the other way: from the running program's memory.
pub(crate) fn copy_from_user_now(dst: &mut [u8], src: u64) -> bool {
    Arch::preempt_disable();
    let copied = Arch::copy_from_user(dst, src);
    sched::preempt_enable();
    copied
}

/// Makes the running program's pages for `len` bytes from `address` ready
/// to be read, or written if `write`, as faults on them would; `false` if
/// one cannot be.
pub(crate) fn fault_in(address: u64, len: usize, write: bool) -> bool {
    let Some(thread) = sched::running() else { return false };
    let Some(user) = thread.user() else { return false };
    user.space.fault_in(address, len, write)
}

/// Lends `len` bytes of the running program's memory from `address` to a
/// device ([`memory::SpaceObject::lend`]): `None` if they cannot be.
pub(crate) fn lend<R>(
    address: u64,
    len: usize,
    write: bool,
    with: impl FnOnce(&[(u64, u32)]) -> R,
) -> Option<R> {
    let thread = sched::running()?;
    let user = thread.user()?;
    user.space.lend(address, len, write, with)
}

/// The system call handler, which the architecture calls by name
/// (`radix_hal::UserMode`, ADR 0406) for every system call: fat LTO
/// inlines it into the entry path.
// The architecture declares this symbol with the same signature, so
// exporting it unmangled is sound.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "Rust" fn radix_syscall(regs: &mut radix_hal::SyscallRegs) {
    syscall::handle(regs);
}

/// The return handler, which the architecture calls by name on the way
/// back to user mode ([`on_return`]).
// As for `radix_syscall`.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "Rust" fn radix_user_return() {
    on_return();
}

/// On the way back to user mode, from a system call, an interrupt or a
/// fault, with interrupts masked: a thread asked to stop ends here, and
/// one with signals to take takes them (`posix::signal::deliver`).
#[inline(always)]
fn on_return() {
    let Some(me) = sched::running() else { return };
    if me.interrupted() {
        returning_interrupted(&me);
    }
}

/// [`on_return`] for a thread with a signal to take or a stop to make:
/// kept out of line, so that every other return skips its frame.
///
/// It takes them with interrupts on, then looks again with interrupts
/// masked, as they stay until user mode: a stop or a signal sent after
/// that look interrupts this CPU in user mode (`sched::interrupt`), where
/// the next way back takes it, but one sent while interrupts were on may
/// have interrupted this CPU here, to no effect. One handler's frame is
/// made on each way back; a signal still to take after it is taken on the
/// handler's own way back, which an interrupt of this CPU to itself brings
/// before the handler's first instruction, so the handlers nest as
/// Linux's do.
#[cold]
#[inline(never)]
fn returning_interrupted(me: &ThreadRef) {
    Arch::enable_interrupts();
    let Some(user) = me.user() else { return };
    if me.should_stop() {
        info!("User: thread {} ({}) stopped", me.id(), me.name());
        note_domain_stop(me, user);
        exit_current(user.clone(), EXIT_STOPPED, Ending::Stop);
    }
    loop {
        let handler = posix::signal::deliver(me, user);
        if me.should_stop() {
            note_domain_stop(me, user);
            exit_current(user.clone(), EXIT_STOPPED, Ending::Stop);
        }
        Arch::disable_interrupts();
        if !me.should_stop() && !me.is_signalled() {
            return;
        }
        if !me.should_stop() && handler {
            Arch::send_ipi(Arch::current_cpu(), Ipi::Reschedule);
            return;
        }
        Arch::enable_interrupts();
    }
}

/// A thread stopped with its domain, by the out-of-memory policy or the
/// domain's `stop`, ends its process to its parent as if killed: as
/// SIGKILL, unless a signal ended it first.
pub(super) fn note_domain_stop(me: &ThreadRef, user: &UserRef) {
    if me.domain().is_stopping() {
        user.process.posix.lock().ended_by.get_or_insert(radix_abi::signal::SIGKILL);
    }
}

/// The first slot of the receive window programs start with, and its size.
const fn default_window() -> u64 {
    ((startup::SLOT_WINDOW as u64) << 32) | startup::WINDOW_SLOTS as u64
}

/// A first-in, first-out queue of waiters, which grows under spin locks
/// (with atomic allocations) and only ever says so when it cannot. Taking
/// from the front costs the same however many wait. (`VecDeque` is not
/// built into the kernel's `alloc`, which leaves out what allocates
/// without saying so when it cannot.)
pub(crate) struct Fifo<T> {
    /// The items from `head` on; the slots before it were taken.
    items: Vec<Option<T>, Kmalloc>,
    head: usize,
}

impl<T> Fifo<T> {
    pub(crate) fn new() -> Fifo<T> {
        Fifo { items: Vec::new_in(HEAP.with(Flags::ATOMIC)), head: 0 }
    }

    /// Adds `item` at the back; gives it back if there is no memory.
    pub(crate) fn push_back(&mut self, item: T) -> core::result::Result<(), T> {
        // Reuse the taken slots before growing: each item moves once per
        // time the queue fills, so a push costs the same on average.
        if self.items.len() == self.items.capacity() {
            self.compact();
        }
        if self.items.try_reserve(1).is_err() {
            return Err(item);
        }
        self.items
            .push_within_capacity(Some(item))
            .map(|_| ())
            .map_err(|item| item.expect("it was pushed as `Some`"))
    }

    /// Makes room for one more item, so that the next `push_back` needs
    /// no memory; says whether there was memory for it.
    pub(crate) fn reserve_one(&mut self) -> bool {
        if self.items.len() == self.items.capacity() {
            self.compact();
        }
        self.items.try_reserve(1).is_ok()
    }

    /// Puts `item` back at the front; right after `pop_front` this needs
    /// no memory. Gives it back if there is no memory.
    pub(crate) fn push_front(&mut self, item: T) -> core::result::Result<(), T> {
        if self.head > 0 {
            self.head -= 1;
            self.items[self.head] = Some(item);
            return Ok(());
        }
        self.push_back(item)?;
        self.items.rotate_right(1);
        Ok(())
    }

    /// Takes the item at the front.
    pub(crate) fn pop_front(&mut self) -> Option<T> {
        let item = self.items.get_mut(self.head)?.take();
        self.head += 1;
        if self.head == self.items.len() {
            self.items.clear();
            self.head = 0;
        }
        item
    }

    /// Keeps only the items `keep` says to.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        self.compact();
        self.items.retain(|item| item.as_ref().is_some_and(&mut keep));
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len() - self.head
    }

    /// The item at the back, the one added last.
    pub(crate) fn back(&self) -> Option<&T> {
        self.items[self.head..].last().and_then(Option::as_ref)
    }

    /// The item at the front.
    pub(crate) fn front(&self) -> Option<&T> {
        self.items.get(self.head).and_then(Option::as_ref)
    }

    /// The items, from the front.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.items[self.head..].iter().flatten()
    }

    /// Takes every item, leaving it empty.
    pub(crate) fn take(&mut self) -> Fifo<T> {
        core::mem::replace(self, Fifo::new())
    }

    /// Drops the taken slots before `head`.
    fn compact(&mut self) {
        let live = self.len();
        self.items.rotate_left(self.head);
        self.items.truncate(live);
        self.head = 0;
    }
}

impl<T> IntoIterator for Fifo<T> {
    type Item = T;
    type IntoIter = core::iter::Flatten<alloc::vec::IntoIter<Option<T>, Kmalloc>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().flatten()
    }
}
