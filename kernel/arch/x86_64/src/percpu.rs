// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Per-CPU data, the global descriptor table and the task-state segment.
//!
//! Every CPU has a [`PerCpu`] in a static array; its GS base register points
//! at its own entry, so `gs:[8]` is the running CPU's index, `gs:[16]` its
//! preemption count and `gs:[20]` its interrupt depth, each changed by one
//! instruction that a migration cannot split. `gs:[24]` is the top of the
//! running thread's kernel stack; `gs:[40]` is the scheduler's word for the
//! running thread, which it reads in one instruction to find the running
//! thread without holding preemption off; `gs:[48]` and `gs:[56]` are the
//! RCX and R11 a system call that restores every register returns with.
//! The boot CPU sets GS base first thing in [`crate::entry`], before any
//! code that asks which CPU it is running on. In user mode GS base is the
//! program's (zero); the entry paths swap it with `swapgs`.
//!
//! Each CPU also has an [`EntryCpu`]: what entering and leaving the kernel
//! needs before it has its own page tables loaded, which under page-table
//! isolation (`crate::pti`) is all of a CPU's data that programs' page
//! tables map. The entries are an array of their own, page-aligned, as
//! long as the [`PerCpu`] array and right after it, so that each CPU's
//! lies at the same distance from its [`PerCpu`]: `gs:[ENTRY + n]` is
//! byte `n` of the running CPU's. It holds a scratch word, which the system
//! call entry uses before it has a stack (`crate::user`), the words the
//! entry and exit paths make the CR3 values of from, the GDT and TSS, and
//! the entry stack.
//!
//! The GDT has the kernel's code and data segments, then the user segments
//! in the order SYSRET needs them. The TSS holds the stack an interrupt
//! from user mode switches to (the running thread's kernel stack, set at
//! every context switch, or under page-table isolation the entry stack,
//! from which the entry moves to the thread's) and the interrupt stack
//! table: double faults, NMIs and machine checks switch to dedicated
//! stacks, so a kernel stack overflow still produces a report.

use core::arch::asm;
use core::cell::UnsafeCell;
use core::ops::Range;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use radix_hal::{Fault, MAX_CPUS};
use radix_sync::{Once, SpinLock, TakeOnce};

use crate::cpu::{self, msr};
use crate::kvm::StealTime;

/// Frames of a stopped CPU's backtrace that the panic report shows.
pub const STOPPED_FRAMES: usize = 12;

/// Code segment selector.
pub const KERNEL_CS: u16 = 0x08;
/// Data segment selector.
pub const KERNEL_DS: u16 = 0x10;
/// The selector SYSRET's user selectors are relative to: user data is 8
/// above it and user code 16 above. It carries RPL 3 itself, because only
/// Intel CPUs force SYSRET's SS selector to RPL 3: AMD ones load it as it
/// is, and a program's SS of 0x20 (RPL 0) under a CS of RPL 3 makes the
/// `iretq` that ends the next interrupt from user mode fault.
pub const USER_BASE: u16 = 0x18 | 3;
/// User data (and stack) segment selector, RPL 3.
pub const USER_DS: u16 = 0x20 | 3;
/// User 64-bit code segment selector, RPL 3.
pub const USER_CS: u16 = 0x28 | 3;
/// Task-state segment selector.
const TSS_SELECTOR: u16 = 0x30;

// SYSRET loads the user selectors from `USER_BASE`; interrupt returns to
// user mode take them from the frame, which the CPU pushed with these.
const _: () = assert!(USER_DS == USER_BASE + 8 && USER_CS == USER_BASE + 16);

/// Offset of [`PerCpu`]'s top-of-kernel-stack word, for the entry code.
pub const KERNEL_RSP_OFFSET: usize = 24;
/// Offsets of [`PerCpu`]'s RCX and R11 for a full return, for the entry
/// code.
pub const FULL_RCX_OFFSET: usize = 48;
pub const FULL_R11_OFFSET: usize = 56;

/// Interrupt stack table slots (1-based, as the IDT names them).
pub const IST_DOUBLE_FAULT: u8 = 1;
pub const IST_NMI: u8 = 2;
pub const IST_MACHINE_CHECK: u8 = 3;
/// Number of IST stacks each CPU has.
pub const IST_STACKS: usize = 3;

/// The kernel stacks of one CPU, for backtraces and overflow reports.
#[derive(Clone, Debug)]
pub struct Stacks {
    /// The ordinary kernel stack.
    pub kernel: Range<usize>,
    /// The interrupt stack table stacks.
    pub ist: [Range<usize>; IST_STACKS],
    /// Guard pages below each stack (unmapped), same order: kernel, then
    /// the IST stacks. Empty ranges where there is none.
    pub guards: [Range<usize>; IST_STACKS + 1],
}

impl Stacks {
    /// The stack containing `addr`.
    pub fn containing(&self, addr: usize) -> Option<Range<usize>> {
        core::iter::once(&self.kernel)
            .chain(self.ist.iter())
            .find(|stack| stack.contains(&addr))
            .cloned()
    }

    /// Whether `addr` is in a guard page.
    pub fn in_guard(&self, addr: usize) -> bool {
        self.guards.iter().any(|guard| guard.contains(&addr))
    }
}

/// The 64-bit task-state segment.
#[repr(C, packed)]
struct Tss {
    _reserved0: u32,
    rsp: [u64; 3],
    _reserved1: u64,
    ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,
    iomap_base: u16,
}

/// A CPU's GDT (null, kernel code and data, an unused slot, user data and
/// code, then the two-slot TSS descriptor) and TSS.
#[repr(C, align(16))]
struct Descriptors {
    gdt: [u64; 8],
    tss: Tss,
}

/// The words of an [`EntryCpu`] that the entry and exit paths use, and its
/// GDT and TSS.
#[repr(C)]
struct EntryHead {
    /// Scratch: the program's stack pointer in the system call entry, RAX
    /// in an exit while it switches page tables.
    scratch: AtomicU64,
    /// What an entry ORs into CR3 as it loads the kernel's view of the
    /// address space: bit 63 where PCIDs are on, which keeps the view's
    /// TLB entries, else 0.
    noflush: AtomicU64,
    /// What an exit ORs into CR3 to load the program's view (`crate::pti`).
    user_view: AtomicU64,
    /// Bit 0: the TLB may hold entries of the program's view that are out
    /// of date, so the next exit loads it without bit 63, flushing them.
    user_flush: AtomicU64,
    /// The top of the entry stack.
    stack_top: AtomicU64,
    descriptors: TakeOnce<Descriptors>,
}

/// What entering and leaving the kernel needs of a CPU before the kernel's
/// page tables are loaded: under page-table isolation the only data of the
/// CPU that programs' page tables map (`crate::pti`). As long as a
/// [`PerCpu`], so that the two arrays keep each CPU's at the same distance.
#[repr(C, align(64))]
pub struct EntryCpu {
    head: EntryHead,
    /// The entry stack: under page-table isolation, where an interrupt or
    /// exception from user mode arrives, and where the exits to user mode
    /// end. Only the processor and the entry and exit paths touch it.
    stack: UnsafeCell<[u8; ENTRY_STACK]>,
}

/// The size of an [`EntryCpu`]'s stack: what is left of a [`PerCpu`]'s
/// size. The entry pushes a [`crate::interrupts::TrapFrame`] on it, 176
/// bytes, before it moves to the thread's stack.
const ENTRY_STACK: usize = size_of::<PerCpu>() - size_of::<EntryHead>();

const _: () = {
    assert!(size_of::<EntryCpu>() == size_of::<PerCpu>());
    assert!(ENTRY_STACK >= 512 && ENTRY_STACK.is_multiple_of(16));
};

// SAFETY: the stack is only written by the processor and the entry and exit
// paths of the CPU it belongs to, with interrupts masked; Rust code never
// reaches it. The other fields are atomics or a `TakeOnce`, which is `Sync`.
unsafe impl Sync for EntryCpu {}

/// Every CPU's [`EntryCpu`], in pages of their own.
#[repr(C, align(4096))]
struct EntryArea([EntryCpu; MAX_CPUS]);

/// The per-CPU data: each CPU's [`EntryCpu`] lies [`ENTRY`] bytes after its
/// [`PerCpu`].
#[repr(C)]
struct Cpus {
    percpu: [PerCpu; MAX_CPUS],
    entry: EntryArea,
}

/// The distance from a CPU's [`PerCpu`] to its [`EntryCpu`].
pub const ENTRY: usize = core::mem::offset_of!(Cpus, entry);
/// Offsets from GS base of the running CPU's [`EntryCpu`] words, for the
/// entry and exit paths.
pub const ENTRY_SCRATCH: usize = ENTRY + core::mem::offset_of!(EntryHead, scratch);
pub const ENTRY_NOFLUSH: usize = ENTRY + core::mem::offset_of!(EntryHead, noflush);
pub const ENTRY_USER_VIEW: usize = ENTRY + core::mem::offset_of!(EntryHead, user_view);
pub const ENTRY_USER_FLUSH: usize = ENTRY + core::mem::offset_of!(EntryHead, user_flush);
pub const ENTRY_STACK_TOP: usize = ENTRY + core::mem::offset_of!(EntryHead, stack_top);
/// How far below the top of the entry stack its user may be.
pub const ENTRY_STACK_SIZE: usize = ENTRY_STACK;

impl EntryCpu {
    const fn new() -> Self {
        Self {
            head: EntryHead {
                scratch: AtomicU64::new(0),
                noflush: AtomicU64::new(0),
                user_view: AtomicU64::new(0),
                user_flush: AtomicU64::new(0),
                stack_top: AtomicU64::new(0),
                descriptors: TakeOnce::new(Descriptors {
                    gdt: [0; 8],
                    tss: Tss {
                        _reserved0: 0,
                        rsp: [0; 3],
                        _reserved1: 0,
                        ist: [0; 7],
                        _reserved2: 0,
                        _reserved3: 0,
                        iomap_base: 0,
                    },
                }),
            },
            stack: UnsafeCell::new([0; ENTRY_STACK]),
        }
    }

    /// The top of the entry stack.
    pub fn stack_top(&self) -> usize {
        self.stack.get() as usize + ENTRY_STACK
    }

    /// Sets what this CPU's entries and exits make CR3 from under
    /// page-table isolation (`crate::pti`).
    pub fn set_views(&self, noflush: u64, user_view: u64) {
        self.head.noflush.store(noflush, Ordering::Relaxed);
        self.head.user_view.store(user_view, Ordering::Relaxed);
    }

    /// Has the next exit to user mode flush the TLB entries of the
    /// program's view.
    pub fn flush_user_view(&self) {
        self.set_user_view_flush(true);
    }

    /// Sets whether the next exit to user mode flushes the TLB entries of
    /// the program's view.
    pub fn set_user_view_flush(&self, flush: bool) {
        self.head.user_flush.store(u64::from(flush), Ordering::Relaxed);
    }

    /// Whether the next exit flushes the program's view, as asked.
    pub fn user_view_flush_pending(&self) -> bool {
        self.head.user_flush.load(Ordering::Relaxed) != 0
    }
}

/// Everything the kernel keeps per CPU.
#[repr(C, align(64))]
pub struct PerCpu {
    /// This structure's own address: `gs:[0]`.
    self_addr: AtomicUsize,
    /// The CPU's index: `gs:[8]`.
    index: AtomicUsize,
    /// The preemption count: `gs:[16]`.
    preempt: AtomicU32,
    /// Interrupt handlers running, nested: `gs:[20]`.
    irq_depth: AtomicU32,
    /// The top of the running thread's kernel stack: `gs:[24]`.
    kernel_rsp: AtomicU64,
    /// Under page-table isolation with PCIDs: a mapping of the kernel's
    /// half was flushed only from the TLB entries of the PCID this CPU ran,
    /// and those of others may still hold it, so the next switch to another
    /// address space flushes them all (`crate::pti`). `gs:[32]`.
    pub kernel_tlb_stale: AtomicBool,
    /// The scheduler's word for the thread running on this CPU:
    /// `gs:[40]`.
    running: AtomicUsize,
    /// RCX and R11 for a system call that returns with every register
    /// restored (`iretq`, not `sysretq`, which takes both for itself):
    /// `gs:[48]` and `gs:[56]`, written with interrupts masked just before
    /// the return (`crate::user`).
    pub full_rcx: AtomicU64,
    pub full_r11: AtomicU64,
    /// The address of this CPU's TSS, once loaded, whose RSP0 follows the
    /// running thread's kernel stack; 0 before, and under page-table
    /// isolation, where RSP0 stays at the entry stack.
    tss: AtomicUsize,
    /// Local APIC ID.
    pub apic_id: AtomicU32,
    /// Listed in the MADT as usable.
    pub present: AtomicBool,
    /// NUMA node, from the SRAT (0 without one).
    pub node: AtomicU8,
    /// Running kernel code.
    pub online: AtomicBool,
    /// Stopped by the panic path.
    pub stopped: AtomicBool,
    /// Where the panic path's NMI stopped this CPU: the interrupted
    /// instruction and its callers, 0 past the last.
    pub stopped_at: [AtomicUsize; STOPPED_FRAMES],
    /// The stack pointer where the panic path's NMI stopped this CPU, for
    /// the crash dump's copy of its stack.
    pub stopped_sp: AtomicUsize,
    /// Timer interrupts taken.
    pub ticks: AtomicU64,
    /// Whether the interrupt being handled came from user mode.
    pub interrupted_user: AtomicBool,
    /// Ping IPIs received.
    pub pings: AtomicU64,
    /// The last TLB shootdown request this CPU has carried out.
    pub tlb_generation: AtomicU64,
    /// The address space this CPU runs (its address), or 0 for the
    /// kernel's own tables.
    pub space: AtomicUsize,
    /// Whether the CPU runs a kernel thread on `space`'s tables, which it
    /// kept from the user thread it ran before
    /// (`AddressSpace::run_kernel_thread`).
    pub lazy: AtomicBool,
    /// Exception handlers running on this CPU, to catch recursion.
    pub exception_depth: AtomicU32,
    /// The fault this CPU is panicking for.
    pub fault: SpinLock<Option<Fault>>,
    /// The CPU's stacks.
    pub stacks: Once<Stacks>,
    /// The stack of the thread running here (start and end), set by the
    /// context switch; both 0 before threads.
    pub thread_stack: [AtomicUsize; 2],
    /// The address of the running thread's context block, or 0.
    pub context: AtomicUsize,
    /// The block of the context whose extended state the registers hold,
    /// or 0 (`thread.rs`).
    pub fpu_holder: AtomicUsize,
    /// FS base as last written here, or `u64::MAX` (no canonical address)
    /// before the first write.
    pub fs_base: AtomicU64,
    /// Reschedule IPIs received.
    pub reschedules: AtomicU64,
    /// When the next periodic tick is due, in nanoseconds.
    pub next_tick_ns: AtomicU64,
    /// The deadline the kernel asked for, or `u64::MAX`.
    pub deadline_ns: AtomicU64,
    /// When the timer is armed to fire, or `u64::MAX` when it is not (or
    /// has fired and its interrupt is not handled yet).
    pub armed_ns: AtomicU64,
    /// Until when the timer also fires every `time::DOORBELL_NS`, since
    /// this CPU answered a TLB shootdown late; 0 when it does not.
    pub doorbell_until_ns: AtomicU64,
    /// Times the timer was written to arm it.
    pub timer_writes: AtomicU64,
    /// Interprocessor interrupts this CPU sent.
    pub ipis_sent: AtomicU64,
    /// The nanoseconds this CPU's TLB shootdowns waited for other CPUs'
    /// answers, and the CPUs they left to the hypervisor to flush instead
    /// (`paging::shootdown_on`).
    pub shootdown_wait_ns: AtomicU64,
    pub shootdowns_left: AtomicU64,
    /// KVM's paravirtual end-of-interrupt word: the hypervisor sets bit 0
    /// when the interrupt it delivers needs no EOI write (`apic`).
    pub pv_eoi: AtomicU32,
    /// KVM's steal-time record of this CPU (`kvm`).
    pub steal_time: StealTime,
}

impl PerCpu {
    const fn new() -> Self {
        Self {
            self_addr: AtomicUsize::new(0),
            index: AtomicUsize::new(0),
            preempt: AtomicU32::new(0),
            irq_depth: AtomicU32::new(0),
            kernel_rsp: AtomicU64::new(0),
            kernel_tlb_stale: AtomicBool::new(false),
            running: AtomicUsize::new(0),
            full_rcx: AtomicU64::new(0),
            full_r11: AtomicU64::new(0),
            tss: AtomicUsize::new(0),
            apic_id: AtomicU32::new(0),
            present: AtomicBool::new(false),
            node: AtomicU8::new(0),
            online: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            stopped_at: [const { AtomicUsize::new(0) }; STOPPED_FRAMES],
            stopped_sp: AtomicUsize::new(0),
            ticks: AtomicU64::new(0),
            interrupted_user: AtomicBool::new(false),
            pings: AtomicU64::new(0),
            tlb_generation: AtomicU64::new(0),
            space: AtomicUsize::new(0),
            lazy: AtomicBool::new(false),
            exception_depth: AtomicU32::new(0),
            fault: SpinLock::new(None),
            stacks: Once::new(),
            thread_stack: [const { AtomicUsize::new(0) }; 2],
            context: AtomicUsize::new(0),
            fpu_holder: AtomicUsize::new(0),
            fs_base: AtomicU64::new(u64::MAX),
            reschedules: AtomicU64::new(0),
            next_tick_ns: AtomicU64::new(0),
            deadline_ns: AtomicU64::new(u64::MAX),
            armed_ns: AtomicU64::new(u64::MAX),
            doorbell_until_ns: AtomicU64::new(0),
            timer_writes: AtomicU64::new(0),
            ipis_sent: AtomicU64::new(0),
            shootdown_wait_ns: AtomicU64::new(0),
            shootdowns_left: AtomicU64::new(0),
            pv_eoi: AtomicU32::new(0),
            steal_time: StealTime::new(),
        }
    }

    /// The CPU's index.
    pub fn index(&self) -> usize {
        self.index.load(Ordering::Relaxed)
    }

    /// Sets the stack that system calls and interrupts from user mode
    /// switch to: the top of the kernel stack of the thread about to run.
    /// With interrupts masked, on this CPU.
    pub fn set_kernel_stack(&self, top: usize) {
        self.kernel_rsp.store(top as u64, Ordering::Relaxed);
        self.write_rsp0(top);
    }

    /// Points RSP0 at `top`, if the TSS is loaded and RSP0 follows the
    /// thread's stack.
    fn write_rsp0(&self, top: usize) {
        let tss = self.tss.load(Ordering::Relaxed);
        if tss != 0 {
            // SAFETY: `tss` is this CPU's TSS, which lives in a static and
            // is only written here, by its own CPU with interrupts masked;
            // the CPU reads RSP0 only when an interrupt or exception
            // arrives from user mode, which cannot happen meanwhile. The
            // TSS is packed, so the write is unaligned.
            unsafe {
                let rsp0 = core::ptr::addr_of_mut!((*(tss as *mut Tss)).rsp[0]);
                rsp0.write_unaligned(top as u64);
            }
        }
    }

    /// Points RSP0 at `top` for good, under page-table isolation: the
    /// entry stack, from which the entry moves to the thread's stack
    /// itself. With interrupts masked, on this CPU, after its TSS is
    /// loaded.
    pub fn pin_entry_stack(&self, top: usize) {
        self.write_rsp0(top);
        self.tss.store(0, Ordering::Relaxed);
    }

    /// This CPU's [`EntryCpu`].
    pub fn entry(&self) -> &'static EntryCpu {
        &CPUS.entry.0[self.index()]
    }

    /// The top of the running thread's kernel stack, where its user
    /// registers are saved when it comes in from user mode.
    pub fn kernel_stack_top(&self) -> u64 {
        self.kernel_rsp.load(Ordering::Relaxed)
    }

    /// The running thread's stack, if it is not one of [`Stacks`].
    pub fn thread_stack(&self) -> Option<Range<usize>> {
        let [start, end] = &self.thread_stack;
        let (start, end) = (start.load(Ordering::Relaxed), end.load(Ordering::Relaxed));
        let stacks = self.stacks.get();
        (start < end && stacks.is_none_or(|s| s.containing(start).is_none())).then_some(start..end)
    }

    /// The stack containing `addr`: one of this CPU's own, or the running
    /// thread's.
    pub fn stack_containing(&self, addr: usize) -> Option<Range<usize>> {
        self.stacks
            .get()
            .and_then(|stacks| stacks.containing(addr))
            .or_else(|| self.thread_stack().filter(|stack| stack.contains(&addr)))
    }

    /// Whether `addr` is in the guard page of one of this CPU's stacks or
    /// of the running thread's.
    pub fn in_stack_guard(&self, addr: usize) -> bool {
        self.stacks.get().is_some_and(|stacks| stacks.in_guard(addr))
            || self.thread_stack().is_some_and(|stack| {
                (stack.start.saturating_sub(4096)..stack.start).contains(&addr)
            })
    }
}

/// Adds one to the running CPU's preemption count.
#[inline(always)]
pub fn preempt_disable() {
    // SAFETY: GS base points at this CPU's `PerCpu` (see `current_index`),
    // and offset 16 is its `preempt` field, an `AtomicU32` only ever
    // changed by its own CPU. One instruction, so it cannot be split by an
    // interrupt or a migration; the asm is a compiler barrier.
    unsafe { asm!("inc dword ptr gs:[16]", options(nostack)) };
}

/// Takes one from the running CPU's preemption count and returns the rest.
#[inline(always)]
pub fn preempt_enable() -> u32 {
    let left: u32;
    // SAFETY: as in `preempt_disable`. Reading the result back separately
    // is fine: a thread is only preempted while the count is 0, and then
    // whichever CPU it runs on next has a count of 0 too.
    unsafe { asm!("dec dword ptr gs:[16]", "mov {:e}, gs:[16]", out(reg) left, options(nostack)) };
    left
}

/// The running CPU's preemption count.
#[inline(always)]
pub fn preempt_count() -> u32 {
    let count: u32;
    // SAFETY: as in `preempt_disable`, reading only.
    unsafe {
        asm!("mov {:e}, gs:[16]", out(reg) count, options(nostack, readonly, preserves_flags));
    };
    count
}

/// Sets the running CPU's preemption count.
pub fn set_preempt_count(count: u32) {
    // SAFETY: as in `preempt_disable`.
    unsafe { asm!("mov gs:[16], {:e}", in(reg) count, options(nostack, preserves_flags)) };
}

/// The scheduler's word for the thread running on this CPU.
#[inline(always)]
pub fn running_word() -> usize {
    let word: usize;
    // SAFETY: as in `preempt_disable`, reading only the `running` field at
    // 40, which only its own CPU writes.
    unsafe { asm!("mov {}, gs:[40]", out(reg) word, options(nostack, readonly, preserves_flags)) };
    word
}

/// Sets the scheduler's word for the thread running on this CPU.
#[inline(always)]
pub fn set_running_word(word: usize) {
    // SAFETY: as in `preempt_disable`, for the `running` field at 40.
    unsafe { asm!("mov gs:[40], {}", in(reg) word, options(nostack, preserves_flags)) };
}

/// Notes that an interrupt handler starts on this CPU.
#[inline(always)]
pub fn interrupt_enter() {
    // SAFETY: as in `preempt_disable`, for the `irq_depth` field at 20.
    unsafe { asm!("inc dword ptr gs:[20]", options(nostack)) };
}

/// Notes that an interrupt handler is done, and returns the depth left.
#[inline(always)]
pub fn interrupt_exit() -> u32 {
    let left: u32;
    // SAFETY: as in `interrupt_enter`.
    unsafe { asm!("dec dword ptr gs:[20]", "mov {:e}, gs:[20]", out(reg) left, options(nostack)) };
    left
}

/// Whether the interrupt the running CPU handles came from user mode
/// (false outside an interrupt).
pub fn interrupted_user() -> bool {
    interrupt_depth() > 0 && current().interrupted_user.load(Ordering::Relaxed)
}

/// The running CPU's interrupt depth.
#[inline(always)]
pub fn interrupt_depth() -> u32 {
    let depth: u32;
    // SAFETY: as in `interrupt_enter`, reading only.
    unsafe {
        asm!("mov {:e}, gs:[20]", out(reg) depth, options(nostack, readonly, preserves_flags));
    };
    depth
}

static CPUS: Cpus = Cpus {
    percpu: [const { PerCpu::new() }; MAX_CPUS],
    entry: EntryArea([const { EntryCpu::new() }; MAX_CPUS]),
};

const _: () = {
    assert!(core::mem::offset_of!(PerCpu, index) == 8);
    assert!(core::mem::offset_of!(PerCpu, preempt) == 16);
    assert!(core::mem::offset_of!(PerCpu, irq_depth) == 20);
    assert!(core::mem::offset_of!(PerCpu, kernel_rsp) == KERNEL_RSP_OFFSET);
    assert!(core::mem::offset_of!(PerCpu, running) == 40);
    assert!(core::mem::offset_of!(PerCpu, full_rcx) == FULL_RCX_OFFSET);
    assert!(core::mem::offset_of!(PerCpu, full_r11) == FULL_R11_OFFSET);
};

/// The data of CPU `index`.
pub fn get(index: usize) -> Option<&'static PerCpu> {
    CPUS.percpu.get(index)
}

/// Where every CPU's [`EntryCpu`] is: whole pages, which nothing else
/// shares.
pub fn entry_area() -> Range<usize> {
    let start = &raw const CPUS.entry as usize;
    start..start + size_of::<EntryArea>()
}

/// Index of the running CPU.
pub fn current_index() -> usize {
    let index: usize;
    // SAFETY: GS base points at this CPU's `PerCpu` from the first
    // instruction of `entry` (boot CPU) or `ap_entry` (others), and offset
    // 8 is its `index` field, an `AtomicUsize` read with a plain load.
    unsafe { asm!("mov {}, gs:[8]", out(reg) index, options(nostack, preserves_flags, readonly)) };
    index
}

/// The running CPU's data, from the address it keeps of itself: one load,
/// where indexing [`CPUS`] takes a bounds check and a multiplication, and
/// system calls and the scheduler ask for it several times each.
pub fn current() -> &'static PerCpu {
    let addr: usize;
    // SAFETY: GS base points at this CPU's `PerCpu` from `set_current` on,
    // which stores the structure's own address at offset 0 (`self_addr`, an
    // `AtomicUsize` read with a plain load) before pointing GS base at it.
    // The address is an element of `CPUS`, a static that lives for ever and
    // is only reached through shared references.
    unsafe {
        asm!("mov {}, gs:[0]", out(reg) addr, options(nostack, preserves_flags, readonly));
        &*(addr as *const PerCpu)
    }
}

/// Points GS base at CPU `index`'s data. Called first thing on each CPU.
pub fn set_current(index: usize) {
    let cpu = &CPUS.percpu[index];
    cpu.self_addr.store(core::ptr::from_ref::<PerCpu>(cpu) as usize, Ordering::Relaxed);
    cpu.index.store(index, Ordering::Relaxed);
    // SAFETY: GS_BASE exists on every x86-64 CPU, and the kernel uses GS
    // only to reach `PerCpu`, which this now points at for this CPU.
    unsafe { cpu::wrmsr(msr::GS_BASE, core::ptr::from_ref::<PerCpu>(cpu) as u64) };
}

fn tss_descriptor(base: u64, limit: u32) -> [u64; 2] {
    let low = u64::from(limit & 0xffff)
        | ((base & 0xff_ffff) << 16)
        | (0x89 << 40) // present, type 9: available 64-bit TSS
        | (u64::from((limit >> 16) & 0xf) << 48)
        | (((base >> 24) & 0xff) << 56);
    [low, base >> 32]
}

/// Loads this CPU's GDT and TSS, with interrupt stacks whose tops are
/// `ist_tops`. Call once per CPU, after [`set_current`].
pub fn load_descriptors(ist_tops: [usize; IST_STACKS]) {
    let cpu = current();
    let entry = cpu.entry();
    entry.head.stack_top.store(entry.stack_top() as u64, Ordering::Relaxed);
    let Some(descriptors) = entry.head.descriptors.take() else {
        panic!("CPU {} loaded its descriptor tables twice", cpu.index());
    };
    let mut ist = [0u64; 7];
    for (slot, top) in ist.iter_mut().zip(ist_tops) {
        *slot = top as u64;
    }
    descriptors.tss = Tss {
        _reserved0: 0,
        rsp: [0; 3],
        _reserved1: 0,
        ist,
        _reserved2: 0,
        _reserved3: 0,
        iomap_base: core::mem::size_of::<Tss>() as u16,
    };
    let tss_base = &raw const descriptors.tss as u64;
    let [tss_low, tss_high] = tss_descriptor(tss_base, core::mem::size_of::<Tss>() as u32 - 1);
    // The user descriptors have DPL 3 and their accessed bits set, so the
    // CPU never writes the table to set them.
    descriptors.gdt = [
        0,
        0x00AF_9A00_0000_FFFF, // 0x08: kernel code, 64-bit
        0x00CF_9200_0000_FFFF, // 0x10: kernel data
        0,                     // 0x18: SYSRET's 32-bit user code, unused
        0x00CF_F300_0000_FFFF, // 0x20: user data
        0x00AF_FB00_0000_FFFF, // 0x28: user code, 64-bit
        tss_low,               // 0x30: the TSS
        tss_high,
    ];
    cpu.tss.store(tss_base as usize, Ordering::Relaxed);

    #[repr(C, packed)]
    struct Pointer {
        limit: u16,
        base: u64,
    }
    let pointer = Pointer {
        limit: (core::mem::size_of::<[u64; 8]>() - 1) as u16,
        base: descriptors.gdt.as_ptr() as u64,
    };
    // SAFETY: the GDT lives in a static that is never written again (the
    // `TakeOnce` was just claimed), its code and data descriptors match the
    // flat 64-bit model the kernel runs in, and the TSS descriptor points
    // at the TSS next to it. Reloading CS through a far return and the data
    // segments keeps every address the same. Loading GS clears its base,
    // which `set_current` sets again right after; this runs before the CPU
    // loads its IDT and takes interrupts, so nothing uses GS meanwhile.
    unsafe {
        asm!(
            "lgdt [{pointer}]",
            "push {cs}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            "mov ds, {ds:x}",
            "mov es, {ds:x}",
            "mov ss, {ds:x}",
            "xor {tmp:e}, {tmp:e}",
            "mov fs, {tmp:x}",
            "mov gs, {tmp:x}",
            "ltr {tss:x}",
            pointer = in(reg) &raw const pointer,
            cs = in(reg) u64::from(KERNEL_CS),
            ds = in(reg) u64::from(KERNEL_DS),
            tss = in(reg) u64::from(TSS_SELECTOR),
            tmp = out(reg) _,
        );
    };
    // Loading GS zeroed its base; point it back at this CPU.
    set_current(cpu.index());
    crate::user::init_cpu();
}
