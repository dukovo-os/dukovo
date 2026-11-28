<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0794. On processors with Meltdown, programs run on page tables without the kernel

- Status: Accepted
- Date: 2026-11-27

## Context

Meltdown (rogue data cache load) lets a program read any memory its page
tables map, the kernel's included: the processor checks the page's
supervisor bit only when the load retires, and the value has reached the
cache by then. Intel's processors from before Cascade Lake and Ice Lake
have it, unless IA32_ARCH_CAPABILITIES says RDCL_NO; AMD's do not. ADR
0790 reports it as `Vulnerable` on those parts, among them the Core
i5-6500 of the lab's OptiPlex 7040.

No microcode or control bit closes it. What closes it is giving the
processor nothing of the kernel's to read while a program runs: page
tables for the program that map only what entering and leaving the kernel
needs, and the kernel's full page tables loaded on every entry. Linux
does this (KPTI), and so do the other kernels that run on these parts.
Its cost is a CR3 write on each crossing, and the TLB entries that the
write throws away; processors with PCIDs keep both sets of entries
across the switch.

## Decision

**Two views of each address space.** Under isolation every address space
has two PML4s side by side in an 8 KiB block: the kernel's view at the
even page, as before, and the program's view at the odd page. The
program's view has the same lower half (each lower-half PML4 entry the
kernel writes is copied to it), and an upper half that maps only:

- the entry text: the interrupt stubs, their common code and the system
  call entry and exits, linked into `.text.entry` in pages of their own;
- each CPU's entry area (`percpu::EntryCpu`): the scratch words that the
  entry and exit paths use, the CPU's GDT and TSS, and a small entry
  stack;
- the IDT, which is a page of its own, read-only;
- the top page of each CPU's NMI, double-fault and machine-check stacks.

Those upper-half tables are shared by every address space: a template
PML4 made at boot reserves their PDPTs, and each new address space copies
its upper half. The kernel's view of every space, the kernel's own page
tables and the firmware's runtime tables are allocated at even pages, so
that setting CR3's bit 12 always selects the program's view.

**PCIDs.** Where the processor has PCIDs, the program's view runs under
the kernel view's PCID with bit 11 set, so PCIDs are 11 bits long
(`radix_mm::Pcids`), and every switch between the views sets CR3's bit 63,
which keeps both views' TLB entries. Without PCIDs each switch flushes the
TLB but for global entries, and none of the kernel's entries is global
under isolation, or they would outlive the switch to the program's view.

**The crossings** are sites of ADR 0791, `Pti`, which the boot CPU turns on
when the plan isolates the kernel:

- the system call entry switches to the kernel's view right after
  SWAPGS, before it touches the thread's kernel stack;
- an interrupt or exception from a program arrives on the CPU's entry
  stack (the TSS's RSP0 points there for good); after saving every
  register it switches views and moves its frame to the top of the
  thread's kernel stack, where it would have arrived without isolation;
- each exit to a program copies the frame it returns through (or the
  program's stack pointer, for SYSRET) to the entry stack, switches to
  the program's view, and returns from there;
- an NMI, machine check or double fault, and any exception in kernel mode,
  can arrive with the program's view loaded, at either end of a crossing.
  Its entry checks CR3's bit 12, switches to the kernel's view if it is
  set, and puts the program's view back on its way out. An exception in
  kernel mode on the entry stack, which is too small for a handler,
  moves to the thread's stack below its top frame and goes back before
  the program's view returns.

**TLB flushes.** A flush of some of a program's pages flushes them from
the running PCID, the kernel's view; under PCIDs the program's view keeps
its own entries for them. They go one by one with INVPCID where the
processor has it; otherwise the exit that next loads the program's view
loads it without bit 63, which flushes the whole view. A flush of kernel
pages under PCIDs reaches only the running PCID, so the CPU flushes every
PCID at its next switch between address spaces; the kernel unmaps its own
pages rarely (the mapping window, stacks freed).

**The flags** are Linux's: `pti=on` isolates the kernel on any processor,
even with `mitigations=off`; `pti=off` and `nopti` never do; `pti=auto`,
the default, isolates it where Meltdown applies and `mitigations` is not
off; `noinvpcid` keeps INVPCID out, as on processors without it. The
Meltdown file reads `Mitigation: PTI` when it is on, and the boot
log says how it runs:

```
Page-table isolation: programs run on page tables without the kernel, both views under PCIDs, a program's pages flushed with INVPCID
```

## Consequences

- Meltdown reads `Mitigation: PTI` on the Intel parts that have it, as
  on Linux.
- Each crossing costs two CR3 writes, and on parts without PCIDs the TLB
  entries a program and the kernel had; system calls, interrupts and
  context switches cost more on those parts. Processors without Meltdown
  pay one taken jump at each site.
- Every address space takes a page more under isolation, and each CPU's
  entry area, kept in the kernel image beside its per-CPU data, holds its
  GDT, TSS and entry stack whether isolation is on or not.
- The kernel's code in the entry text is everything a program's view maps
  of the kernel's code; what the entry and exit paths reach before the
  switch must be there or in the entry area. A new entry path goes into
  `.text.entry`.
- The cpu-pti boot test runs an emulated Skylake, which has Meltdown and
  no PCIDs, under QEMU's emulation, whatever the host. QEMU's emulation
  has no PCIDs, so cpu-pti-pcid and cpu-pti-invpcid run the host's own
  processor under KVM with `pti=on`, the first with `noinvpcid`, which
  Linux has too; on a host without PCIDs, or without KVM, they check
  isolation without them.
