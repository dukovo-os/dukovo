<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0770. A kernel thread runs on the tables of the program before it

- Status: Accepted
- Date: 2026-11-26

## Context

A switch to a kernel thread, the idle thread most often, loaded the
kernel's own tables, and the switch back to a program loaded the
program's. With PCIDs the program's TLB entries survive both, tagged
with its identifier. Without them every load of CR3 flushes
the TLB but for the kernel's global entries, and the program refills its
entries on its next accesses. A server's threads sleep between requests,
so each request on an otherwise idle CPU costs a round trip to the idle
thread and a full refill.

The lab's virtual machine runs on an AMD Ryzen 9 9900X3D, which has no
PCIDs (its Linux lists none, and Radix's boot line in the guest says
"PCID off"), and under a hypervisor's nested paging each refill walks
two sets of tables. A profile of the PostgreSQL select row there, at
2.09 times Linux's time, counted about the same instructions in the
backend as Linux's but 2.4 times the cycles and 2.4 times the L2 misses,
and 2.8 untagged switches of the tables for each transaction. Linux in
the same virtual machine runs kernel threads on a *lazy TLB*: the idle
thread keeps the tables of the program before it, and a switch back to
that program loads nothing.

## Decision

- **A kernel thread keeps the tables loaded**
  (`AddressSpace::run_kernel_thread`). The context switch to a thread with no user space loads nothing and
  marks the CPU *lazy* (`PerCpu::lazy`); the CPU still marks the space
  as running, so it still gets the space's shootdowns. A switch back to
  the same space reloads nothing and keeps the TLB; a switch to another
  loads that one's tables, as before. Any explicit switch clears the
  mark.
- **A lazy CPU leaves at its first shootdown of a user range**
  (`paging::serve_shootdown`): it flushes the range and switches to the
  kernel's tables, so the space's later changes no longer interrupt it.
  As on Linux, a space that runs on other CPUs while this one idles
  costs this one interrupt at most; with PCIDs the switch back flushes
  the space's identifier, its TLB generation having moved.
- **A space's tables go only once no CPU has them loaded.** Its last
  handle no longer means that no CPU runs it: a lazy CPU may. So the
  last handle's drop first takes the space off the lazy CPUs
  (`AddressSpace::leave_lazy_cpus`): this CPU at once if it is one, the
  others by a shootdown each answers by switching (`paging::leave_on`).
  That shootdown interrupts every target, even one the host has
  stopped, which the paravirtual flush would otherwise leave to the host
  (ADR 0105): the host's flush empties a TLB but leaves CR3 where it
  was. `destroy` still checks that no CPU marks the space as running.
- **A CPU clears its mark on a space after loading other tables**, where
  it cleared it before: while the mark is set, the space's tables stay,
  and they must stay as long as CR3 names them.

## Alternatives

- **The lazy CPU holds a handle on the space**, as Linux's `mmgrab`.
  The handle's drop on the switch to the next program could be the last
  one, and would free the space's page tables in the scheduler, with
  interrupts off and the run queue's lock held, or need a place to put
  it off to.
- **Lazy CPUs left out of shootdowns**, flushing when they switch back
  if the space's generation moved, as Linux does for changes that free no
  page table. Fewer interrupts to idle CPUs, but a change that frees page
  tables must still reach them, which every shootdown here would have to
  say. Leaving at the first one keeps one kind of request.
- **PCIDs in the virtual machine.** The host lists none to pass on,
  and CR4.PCIDE faults where CPUID lacks the feature.

## Consequences

- A CPU that idles between two runs of the same program loads no tables
  and keeps the program's TLB entries.
- A program's change to its mappings may interrupt a CPU that idles
  after running it, once; before, an idle CPU was never interrupted for
  a program's changes.
- A process's end may interrupt the CPUs where it ran last, once each,
  before its tables are freed.
- Kernel threads run with a program's mappings in the lower half. SMAP
  and SMEP still fault on any access to them, and a kernel thread has no
  business there.
