<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0740. Threads go to performance cores first on processors whose cores differ

- Status: Accepted
- Date: 2026-11-25

## Context

Intel's processors from the 12th generation Core on (Alder Lake, Raptor
Lake and later) have cores of two kinds. Performance cores are fast and
have two hardware threads each. Efficient cores are slower and smaller,
and have one thread each. The lab's workstation has a Core i9-12900, with
8 performance cores and 8 efficient ones, which make 24 CPUs. CPUID leaf
7's EDX bit 15 says that a processor has cores of more than one kind, and
the top byte of leaf 1Ah's EAX gives each CPU its own kind: 40h for a
performance core and 20h for an efficient one.

The scheduler treated every CPU alike. A waking thread went back to the
idle CPU it last ran on, or to the nearest idle one. Balancing compared
counts of threads and never moved a thread running alone on its CPU. On
such a processor, a single-threaded program that once ran on an
efficient core stayed there while performance cores sat idle. A program
with fewer threads than the machine has CPUs landed on whichever kind
each thread happened to wake on. Speed tests beside Linux were then
measuring where threads landed, not what Dukovo does.

Linux handles these processors with ITMT (Intel's Turbo Boost Max
Technology 3.0 support). Each CPU gets a priority, highest for a
performance core's first thread, then the efficient cores, then the
performance cores' second threads. Idle CPUs of a higher priority then
pull work from lower ones, including a task running alone ("asym
packing"). Linux also reads the hardware feedback interface's table, but
only to report it to programs. It does not use Thread Director's
per-thread classes to place tasks.

## Decision

- **Each CPU says what kind of core it is.** The HAL gains
  `Cpu::core_kind`, which reads CPUID leaf 1Ah on x86-64 when leaf 7 says
  the cores differ (`CoreKind::of_intel`). Each CPU records its kind as it
  starts scheduling. AArch64 and RISC-V say `Only` until those releases
  read their own descriptions of core types.
- **Ranks, once every CPU is up.** If the online CPUs include both kinds,
  `sched::rank_cores` gives each CPU a rank: 3 for a performance core's
  first hardware thread, 2 for an efficient core, and 1 for a performance
  core's other threads. A second thread shares its core's execution units
  with the first, and the first is busy whenever the second is chosen
  after it, so an efficient core of its own does more. The topology is
  then *ranked*, and the boot log says so. Ranks come from the kind of
  core alone: the small differences in highest frequency between
  performance cores, which Linux also ranks, are left out.
- **Waking threads go to the fastest idle CPU.** On a ranked topology,
  `select_cpu` and `select_rt_cpu` look only at the idle CPUs of the
  highest rank the thread may use, and take the nearest of them by level
  and node distance. The shortcut that sends a thread back to its idle
  last CPU without reading other CPUs' loads holds only when that CPU is
  of the top rank.
- **A lone thread on a slower CPU moves up.** On each tick of a busy
  slower CPU, if the running thread is alone there, has run for 4 ms
  (`MOVE_UP_NS`), and a faster CPU it may use is idle (`faster_idle`),
  the CPU asks for a switch. At that preemption, if the thread is still
  alone and the target is still idle and allowed, the thread leaves for
  the target, as a thread whose affinity changed does. This is the
  counterpart of Linux's asym packing. Balancing itself is unchanged and
  still counts threads alike on every CPU. `/processes/stat` counts the
  moves as `moves_up`.
- **Equal ranks change nothing.** When every rank is the same, a ranked
  topology chooses as an unranked one does, which a property test checks.
  On a processor whose cores are all of one kind, which is every machine
  before this one, nothing is ranked, and every path runs as before.
- **`hybrid=off`** leaves the ranks unset and places the CPUs alike. A
  run can then measure the processor both ways.

## Consequences

- Single-threaded and lightly threaded work runs on performance cores
  when they are free, as it does on Linux, so the speed tests compare
  like with like on the workstation.
- A thread moves only at a tick, after running alone for 4 ms, and only
  to an idle faster CPU. Moves therefore cannot bounce a thread between
  CPUs: each one goes up in rank. Two slower CPUs can choose the same idle
  CPU at once. The second thread then waits there until balancing moves
  it, which is the same race two wakers already have.
- Balancing does not weigh CPUs by speed. With more runnable threads
  than CPUs, a performance core and an efficient core each get the same
  number of threads. Weighing by capacity is the next step if
  measurements show the need.
- Thread Director's hints are not read. They tell which threads gain most
  from a performance core, and how the cores' speeds change with
  temperature and power limits. They belong with energy-aware scheduling
  on heterogeneous CPUs, in release 1.29.
- QEMU cannot present cores of two kinds, so ranking is tested on the
  host (`radix-sched`'s topology tests). The boot tests show that nothing
  changes on uniform CPUs. The first run on cores of two kinds is the
  workstation's lab run, with and without `hybrid=off`.
