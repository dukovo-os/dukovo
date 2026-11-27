<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0797. The scheduler keeps threads within a last-level cache before it moves them across a package

- Status: Accepted
- Date: 2026-11-27

## Context

The scheduler's topology (`radix_sched::Topology`, ADR 0654) knew each
CPU's core, package and NUMA node, and took a package's cores to share
its last-level cache. Many processors split a package between several
last-level caches:

- AMD's Zen processors group their cores into core complexes, each with
  an L3 of its own: four cores to a complex on Zen 1 and Zen 2, eight
  from Zen 3 on. A Ryzen 9 has two or more of them, and an EPYC up to
  sixteen. Data in one complex's L3 is as far from the others as memory.
- Intel's processors with several dies in a package (Cascade Lake-AP,
  and the server processors that followed it) give each die its L3.

On these, placement and balancing treated the whole package as one
cache: a waking thread went to any idle CPU of the package, though one
sharing its cache was idle too, and balancing moved threads between
caches as readily as within one, at the package level's interval and
with its half-millisecond window for warm caches.

The APIC IDs came from CPUID's leaf 0xB alone, whose levels are the
hardware threads and the cores. On Intel's processors with dies, the
cores' level holds a die rather than the package, so each die was taken
for a package. Leaf 0x1F, its successor, names the modules, tiles and
dies between the cores and the package. AMD's processors before Zen 2
have neither leaf, and the package's width came from leaf 1's count of
logical processors, which on parts with disabled cores can be narrower
than the APIC IDs' field (AMD numbers a complex's four core places even
where only three cores are enabled).

## Decision

**Reading the layout.** The HAL decodes an APIC ID's fields from CPUID
(`radix_hal::topology::ApicLayout`): the hardware threads' shift and the
package's from leaf 0x1F where the processor fills it in, else leaf 0xB,
else, on AMD's, the package's APIC ID width in leaf 0x8000_0008 with the
threads of a core from 0x8000_001E, else leaf 1's count. The CPUs that
share the last-level cache come from the deterministic cache parameters,
Intel's leaf 4 and AMD's 0x8000_001D (with the topology extensions): of
the caches holding data, the one of the highest level, and the number of
APIC IDs that share it. A cache said to be shared by fewer IDs than a
core has, or more than a package has, is taken as the core's or the
package's. Only leaves the processor lists are read, since Intel's
processors answer a leaf beyond their highest with that one's registers.

**The cache level.** A CPU's place gains its last-level cache, numbered
within its package, and the topology a level for it between the core
and the package: the CPUs of a core, of a last-level cache, of a
package, of a node, and the machine. As before, a level that adds no
CPUs to the one below is skipped, so on a processor with one cache per
package the cache level stands where the package level did, with the
same interval and window, and the package level is skipped. Where a
package has several caches:

- a waking thread takes an idle CPU of its own cache before one of
  another cache in the package;
- balancing tries within the cache every 2 ticks and across the package
  every 4 (the node's 8 and the machine's 16 are unchanged), and leaves a
  thread that ran within the last 2 ms where it is when moving it would
  cross caches: its data in the cache it would leave is still worth more
  than an even spread for a moment longer.

The boot log adds a line that counts the hardware threads, cores,
last-level caches, packages and NUMA nodes, and lscpu reads leaf 0x1F
before 0xB for its sockets and cores.

## Consequences

- On Zen processors with several core complexes, and Intel's with
  several dies, threads that share data stay within one L3 unless the
  load is uneven between caches, as Linux's scheduler domains keep them
  with its cache-sharing level.
- Processors with one cache per package behave as before: the cache
  level takes the package level's place. These include every Intel
  client processor and every machine in the lab today, the M75q's Ryzen
  5 PRO 5650GE (one complex of six cores) among them.
- The balancer's counts of threads pulled, in kdb's `sched` and in the
  sched-balance self-test, have a fifth entry, for the cache level.
- The cpu-topology boot test runs an emulated Skylake server package of
  two dies of four cores, each with its L3, through leaves 0x1F and 4.
  QEMU's emulation offers no topology extensions on AMD's models, so
  AMD's cache leaf is checked by the HAL's host tests, on the layouts of
  Zen 1 and Zen 3, and on hardware. The R740's two Xeon Gold 6152s have
  one L3 each, so the lab's proof of a split package waits for a Ryzen or
  EPYC with several complexes.
- Leaf 0x8000_0026, which AMD's Zen 4 and later add to name complexes and
  dies, and the Intel hybrid processors' modules of four efficient cores
  sharing an L2, are not read: neither changes the last-level cache.
