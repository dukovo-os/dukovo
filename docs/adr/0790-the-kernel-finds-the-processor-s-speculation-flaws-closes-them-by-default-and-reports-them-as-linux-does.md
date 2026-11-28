<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0790. The kernel finds the processor's speculation flaws, closes them by default, and reports them as Linux does

- Status: Accepted; retpolines and return thunks since [0793](0793-the-kernel-s-indirect-branches-and-returns-go-through-thunks-that-the-boot-cpu-rewrites-for-the-processor.md), page-table isolation since [0794](0794-on-processors-with-meltdown-programs-run-on-page-tables-without-the-kernel.md), copies from programs clamp their range instead of fencing since [0803](0803-a-copy-from-or-to-a-program-clamps-its-range-instead-of-fencing-after-its-check.md)
- Date: 2026-11-27

## Context

Since 2018 every x86 processor has been found to leak data through work it
does speculatively and then throws away: Meltdown, Spectre v1 and v2,
Speculative Store Bypass, L1TF, the MDS family (MDS, TAA, MMIO stale data,
RFDS), SRBDS, Retbleed, SRSO, GDS, BHI, ITS, TSA, and more. Each has a
fix in microcode, in the kernel, or both. A program that uses them reads
the kernel's memory, or another program's, through the cache.

Radix did none of this. It read no flaw's bits, set no control register,
and told nobody which flaws the machine had. Linux's answer is the one
that programs, administrators and scanners know: it decides per flaw from
the processor's model and its "not affected" bits, closes each by default,
takes `mitigations=` and a flag per flaw, and reports each in a file of
`/sys/devices/system/cpu/vulnerabilities`.

## Decision

**Detection is a library of facts.** `radix-cpuflaws` (`kernel/lib/
cpuflaws`) is a host-tested crate with no `unsafe`. It takes a `Cpu`: the
vendor, family, model, stepping and microcode revision; CPUID leaves 1, 7,
7.2, 8000_0008h and 8000_0021h; IA32_ARCH_CAPABILITIES; IA32_MCU_OPT_CTRL;
whether cores run two threads; the physical address bits and the top of
RAM. `find` says which flaws it has, from the per-model traits of Intel's,
AMD's and Hygon's parts and from every "not affected" bit the processor
reports, which win over the model. The model tables hold the same facts as
Linux's, which is where processors' makers publish them; the code is
Radix's own.

**Every flaw is closed by default, where the processor allows it.** `Plan`
decides how, given what the kernel was built with and what the command
line asks:

- Spectre v1: copies from programs fence after their access check, the
  interrupt entry fences after its SWAPGS choice where programs could
  point GS anywhere, and every index a program names goes through
  `radix_sync::nospec::index` after its bounds check (system call
  numbers, capability slots and the other tables a program indexes, as in
  docs/coding-standard.md).
- Spectre v2 and Retbleed: Intel's enhanced IBRS, AMD's automatic IBRS, or
  IBRS while in the kernel on Intel parts without them. AMD's Zen 1 to 3
  need retpolines and return thunks, which arrive patched in at boot in a
  later change; until then those report `Vulnerable`, and
  `spectre_v2=ibrs`, `retbleed=ibpb` and `spec_rstack_overflow=ibpb` close
  them at a higher cost.
- BHI: BHI_DIS_S where the processor has it, else the branch history is
  cleared on each system call. ITS is closed with the same thunks as
  Retbleed's.
- Speculative Store Bypass: left open by default, as on Linux, whose
  default protects only programs that ask; `spec_store_bypass_disable=on`
  closes it for everything.
- MDS, TAA, MMIO stale data, RFDS and TSA: VERW clears the processor's
  buffers on every exit to a program, and before idle where the other
  hardware thread could see them; TSX is turned off wherever TSX_CTRL can.
- SRBDS and GDS: the microcode's own protection is left on (and set where
  firmware turned it off), never touching a bit the firmware locked.
- L1TF: entries that are not present keep their frame or swap slot
  inverted (below).
- Zen 1's division by zero and Zenbleed: the divider's state is cleared on
  exit, and the chicken bit is set where the microcode does not fix
  Zenbleed.
- Flaws that only a guest can use (L1TF's VMX side, iTLB multihit, VMSCAPE,
  PBRSB) report that no guests run: Radix runs none until 1.23. The plan
  takes `Built::guests`, so the host side is chosen when it does.

**Second hardware threads stay on.** `nosmt` and `mitigations=auto,nosmt`
keep them off; the default leaves them on, as Linux does, and the report
says where that leaves a flaw open (`SMT vulnerable`).

**The flags are Linux's.** `mitigations=`, `nosmt`, `nospectre_v1`,
`spectre_v2=`, `nospectre_v2`, `spectre_v2_user=`,
`spec_store_bypass_disable=`, `nospec_store_bypass_disable`, `retbleed=`,
`spec_rstack_overflow=`, `tsx=`, `mds=`, `tsx_async_abort=`,
`mmio_stale_data=`, `reg_file_data_sampling=`, `tsa=`, `srbds=` and
`gather_data_sampling=`, with the values dukovo-boot(7) lists. A value the
kernel does not take is logged and the flag left at `auto`.
`mitigations=off` turns off everything that its own flag does not ask for.

**The report is Linux's.** `/kernel/devices/system/cpu/vulnerabilities/`
holds one file per flaw, with Linux's name and Linux's words, so that
`lscpu`, scanners and the health check read it as they read Linux's. The
boot log says what was found and what the kernel does about it.

**L1TF.** A processor with L1TF reads ahead from its first-level cache at
the address in a page-table entry that is not present, before it faults.
Entries that hold a frame without access (`PROT_NONE`) keep the frame's
bits inverted, and swapped-out pages' entries keep their slot's page
inverted with the slot's area in bits 52 to 59, as Linux keeps both: the
address they name is in the upper half of the physical addresses, where
no RAM is. Two limits come with it. A swap area on such a processor uses
at most 2^(*b*-13) pages, *b* being the cache's address bits (44 on Intel's
cores from Nehalem to Kaby Lake, whose caches hold more bits than their
addresses reach, else the physical address bits): 8 TiB on Skylake. And a
machine whose RAM reaches into the upper half reports `Vulnerable`, as
Linux does.

## Consequences

- Programs pay for the protections: an exit to a program costs a VERW on
  most Intel parts from before 2019, and IBRS on Skylake-era parts costs
  more. The benchmark gate (ADR 0039) compares Radix with Linux both at
  their defaults, so neither side is measured without its protections.
- AMD's Zen 1 to 3 and Meltdown-affected Intel parts report some flaws as
  `Vulnerable` until the retpolines and KPTI arrive (docs/deferred.md).
- A new flaw means a new model trait, a new `Flaw`, a new line in the plan
  and a host test; the report and the flags follow from the tables.
- Swap areas over the limit on an L1TF processor are cut to it, with a
  warning.
