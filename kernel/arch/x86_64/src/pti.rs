// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Page-table isolation (ADR 0794): on processors with Meltdown, programs
//! run on page tables that map almost nothing of the kernel, so that there
//! is nothing of it for a speculative read to reach.
//!
//! Every address space then has two PML4s, side by side in an 8 KiB block:
//! the kernel's view at the even page, which maps the kernel's half as the
//! kernel's own tables do, and the program's view at the odd page, whose
//! lower half is the same (its PML4 entries are copies of the kernel
//! view's, [`mirror`]) and whose upper half maps only what entering and
//! leaving the kernel needs: the entry text (the interrupt stubs and the
//! system call entry and exits), each CPU's [`percpu::EntryCpu`] (its
//! scratch words, GDT, TSS and entry stack), the IDT, and the top page of
//! each CPU's interrupt stacks. Those tables are the same for every
//! address space ([`TEMPLATE`]).
//!
//! The entry paths switch CR3 from the program's view to the kernel's
//! first thing, and the exit paths back last thing, at sites that the boot
//! CPU turns on (`crate::speculation`): CR3 with bit 12 set is the
//! program's view. Where the processor has PCIDs, the program's view runs
//! under the kernel view's PCID with bit 11 set, and both keep their TLB
//! entries across the switch (CR3's bit 63); PCIDs are then 11 bits long
//! (`radix_mm::Pcids`). An interrupt from user mode arrives on the entry
//! stack (the TSS's RSP0), and the entry moves its frame to the thread's
//! kernel stack; an exit copies the frame it returns through to the entry
//! stack before it switches. An NMI, machine check or double fault may
//! arrive in the kernel while CR3 is still or already the program's view;
//! its entry looks at CR3, switches if it must, and switches back on its
//! way out.
//!
//! The kernel's mappings are not global under isolation, or they would
//! stay in the TLB across the switch to the program's view. A flush of
//! some of a program's pages flushes them from the program's view too: one
//! by one with `invpcid` where the processor has it, or the whole view at
//! the next exit ([`flushed`]). A flush of some of the kernel's pages with
//! PCIDs flushes them from the running PCID only, so the next switch to
//! another address space flushes every PCID.

use core::ops::Range;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use radix_sync::SpinLock;

use crate::memory::{self, PAGE};
use crate::paging::{self, Access};
use crate::{cpu, percpu};

/// CR3's bit that selects the program's view: its PML4 is the page after
/// the kernel view's.
pub const USER_VIEW: u64 = 0x1000;
/// CR3's PCID bit of the program's view, where PCIDs are on.
pub const USER_PCID: u64 = 0x800;
/// CR3's bit that keeps the TLB entries of the PCID loaded.
const NOFLUSH: u64 = 1 << 63;

/// Whether the kernel isolates itself.
static ON: AtomicBool = AtomicBool::new(false);
/// Whether a flush of a program's pages flushes them from its view with
/// `invpcid`, rather than the whole view at the next exit.
static INVPCID: AtomicBool = AtomicBool::new(false);
/// Whether INVPCID may be used where the CPU has it (no `noinvpcid`).
static INVPCID_ALLOWED: AtomicBool = AtomicBool::new(true);
/// The physical address of a PML4 whose upper half every program's view
/// copies: its PDPTs are made at [`init`] and shared by all of them, so
/// what [`share`] maps later reaches every view.
static TEMPLATE: AtomicU64 = AtomicU64::new(0);
/// Held while the shared tables change.
static LOCK: SpinLock<()> = SpinLock::new(());

unsafe extern "C" {
    static __entry_text_start: u8;
    static __entry_text_end: u8;
}

/// Whether the kernel isolates itself from programs' page tables.
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Builds the program's view of the kernel and stops making the kernel's
/// mappings global. Called once, by the boot CPU, when the plan asks for
/// isolation (`crate::speculation`), after its IDT, descriptors and
/// interrupt stacks are set up, before the other CPUs start and before any
/// address space is made.
pub fn init() {
    paging::drop_global();
    let template = paging::new_root(false).expect("out of memory for page tables");
    // Every slot of the PML4 that anything shared lies in: the kernel's
    // image, and the mapping window, where the other CPUs' stacks go.
    let window = memory::window();
    for virt in [memory::kernel_base(), window, window + memory::WINDOW_SIZE - 1] {
        paging::reserve_slot(template, virt);
    }
    TEMPLATE.store(template, Ordering::Relaxed);
    let symbol = |name: &u8| core::ptr::from_ref(name) as u64;
    // SAFETY: the linker script defines both symbols, page-aligned, around
    // the entry text; only their addresses are taken.
    let text = unsafe { symbol(&__entry_text_start)..symbol(&__entry_text_end) };
    share(text, Access::Code);
    let entry = percpu::entry_area();
    share(entry.start as u64..entry.end as u64, Access::ReadWrite);
    let idt = crate::interrupts::idt_page();
    share(idt.start as u64..idt.end as u64, Access::ReadOnly);
    if let Some(stacks) = percpu::current().stacks.get() {
        share_stack_tops(&stacks.ist);
    }
    let pcid = paging::PCID.load(Ordering::Relaxed);
    let invpcid = pcid && cpu::feature::invpcid() && INVPCID_ALLOWED.load(Ordering::Relaxed);
    INVPCID.store(invpcid, Ordering::Relaxed);
    ON.store(true, Ordering::Relaxed);
    radix_klog::info!(
        "Page-table isolation: programs run on page tables without the kernel, {}",
        match (pcid, invpcid) {
            (true, true) => "both views under PCIDs, a program's pages flushed with INVPCID",
            (true, false) => "both views under PCIDs, a program's view flushed whole on exit",
            _ => "without PCIDs",
        }
    );
}

/// Whether INVPCID may flush a program's pages where the CPU has it:
/// false with the `noinvpcid` flag. Called once, by the boot CPU, before
/// [`init`].
pub fn set_invpcid_allowed(allowed: bool) {
    INVPCID_ALLOWED.store(allowed, Ordering::Relaxed);
}

/// Sets this CPU's words for the entry and exit paths and points its RSP0
/// at its entry stack. Every CPU runs it, after its descriptors are loaded.
pub fn init_cpu() {
    if !on() {
        return;
    }
    let cpu = percpu::current();
    let entry = cpu.entry();
    if paging::PCID.load(Ordering::Relaxed) {
        entry.set_views(NOFLUSH, USER_VIEW | USER_PCID);
    } else {
        entry.set_views(0, USER_VIEW);
    }
    cpu.pin_entry_stack(entry.stack_top());
}

/// Maps the top page of each of `stacks`, interrupt stacks, into every
/// program's view: an NMI, machine check or double fault arrives there,
/// and its entry switches to the kernel's view before it goes further.
/// Called before the CPU they belong to starts.
pub fn share_stack_tops(stacks: &[Range<usize>]) {
    if TEMPLATE.load(Ordering::Relaxed) == 0 {
        return;
    }
    for stack in stacks {
        let top = stack.end as u64;
        share(top - PAGE..top, Access::ReadWrite);
    }
}

/// Maps the kernel's pages in `range` into every program's view.
fn share(range: Range<u64>, access: Access) {
    let template = TEMPLATE.load(Ordering::Relaxed);
    let _guard = LOCK.lock();
    paging::share_kernel_pages(template, range, access);
}

/// Copies the shared upper half into the program's view of a new address
/// space, whose kernel view's PML4 is at `pml4`.
pub(crate) fn fill_user_view(pml4: u64) {
    let template = paging::table(TEMPLATE.load(Ordering::Relaxed));
    for (mine, theirs) in paging::table(pml4 | USER_VIEW)[256..].iter().zip(&template[256..]) {
        mine.store(theirs.load(Ordering::Relaxed), Ordering::Relaxed);
    }
}

/// Copies `value`, just written to entry `index` of the PML4 at `root`, to
/// the program's view, if `root` is an address space's kernel view and the
/// entry is in the lower half.
pub(crate) fn mirror(root: u64, index: usize, value: u64) {
    if on() && index < 256 && root != paging::kernel_pml4() {
        paging::table(root | USER_VIEW)[index].store(value, Ordering::Relaxed);
    }
}

/// What [`paging::flush_local`] flushed from the running PCID.
pub(crate) enum Flushed {
    /// These pages.
    Pages(Range<u64>),
    /// The whole running PCID, but for global entries.
    Space,
    /// Everything, every PCID's.
    All,
}

/// Sees to what isolation needs after `flush` flushed the running PCID,
/// for a range in the lower half if `user`: the program's view of the
/// running address space keeps its own entries of the lower half, and with
/// PCIDs the other address spaces' kernel views keep theirs of the upper.
/// Without PCIDs every switch of CR3 flushes everything.
pub(crate) fn flushed(user: bool, flush: Flushed) {
    if !on() || !paging::PCID.load(Ordering::Relaxed) {
        return;
    }
    let cpu = percpu::current();
    match flush {
        Flushed::All => cpu.kernel_tlb_stale.store(false, Ordering::Relaxed),
        Flushed::Pages(pages) if user && INVPCID.load(Ordering::Relaxed) => {
            let pcid = (cpu::cr3() & 0x7ff) | USER_PCID;
            for page in pages.step_by(PAGE as usize) {
                cpu::invpcid_address(pcid, page as usize);
            }
        }
        Flushed::Pages(_) | Flushed::Space if user => cpu.entry().flush_user_view(),
        Flushed::Pages(_) | Flushed::Space => {
            cpu.kernel_tlb_stale.store(true, Ordering::Relaxed);
        }
    }
}
