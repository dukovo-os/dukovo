<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0796. CPUs without HWP or CPPC run at the firmware's P-states, which the kernel picks from the load

- Status: Accepted
- Date: 2026-11-27

## Context

ADR 0202 has CPUs with Intel's HWP or AMD's CPPC registers pick their
own speed within the levels the kernel asks for. Every other CPU kept
whatever speed the firmware left it at: Intel's processors from before
Skylake, and later ones whose firmware keeps HWP off (servers set to let
the operating system manage power often do); AMD's Zen 1 and Zen 2, and
later ones whose firmware does not offer CPPC. Such a CPU usually runs at
its nominal frequency throughout: it never boosts, and it never slows
down when idle or lightly loaded.

These CPUs change speed only when told to. The firmware lists their
performance states in each processor object's `_PSS` (ACPI 6.5, 8.4.5):
for each, its frequency and the value that chooses it, fastest first;
`_PCT` names the registers the value is written to and the state read
from; `_PPC` keeps the fastest few off limits, for a power supply or a
thermal design that cannot carry them. On Intel's processors with
Enhanced SpeedStep the registers are IA32_PERF_CTL and IA32_PERF_STATUS,
whose bits 15:8 are the state's bus ratio; Intel's firmware lists the
turbo range as a state 1 MHz above the nominal one, whose ratio is the
highest turbo ratio. On AMD's with hardware P-states they are
MSRC001_0062 and MSRC001_0063, and the value is the state's number.

## Decision

**Taking the states.** When the processor objects are read for their
idle states (ADR 0795), the kernel also reads `_PCT`, and where both its
registers are functional fixed hardware, `_PSS` and `_PPC`. Each CPU
takes its states on itself if it has neither HWP nor CPPC, has the
register (CPUID's Enhanced SpeedStep bit, or its hardware P-state bit on
AMD's), and is no virtual machine, whose host picks the speed. States
are levels 100 MHz apart, as HWP's are: an Intel state's level is its
ratio, an AMD state's its frequency in hundreds of megahertz. The lowest
level is the slowest state's, the nominal level the fastest state's that
is not the turbo range, and the highest the fastest allowed one's. CPUs
with the same states share one table.

**The control.** Such CPUs get the speed control of ADR 0202 with a third
driver, the firmware's P-states, named `acpi-cpufreq` in `scaling_driver`
after Linux's driver for them. Requests work as before: `cpu.boost` keeps
the CPU at the nominal level or below, `cpu.speed` holds it at the top of
its range, passive cooling caps it. A request's desired level, or its
highest where it has none, is turned into the slowest state at or above
it, never faster than the highest allowed, and written only when it
differs from the state the CPU was last put in.

**The governor.** Since the CPU does not pick, the kernel does, at each
CPU's tick (100 Hz), on the CPU itself. From the CPU's idle time it
works out how busy the CPU was since the last tick; from APERF and MPERF,
which count at the CPU's own frequency and at the nominal one's while it
runs, the level it ran at meanwhile. The work done is the product of
the two; the level asked for is the one at which that work would keep
the CPU busy four fifths of the time, so that a little more work does
not saturate it, and a CPU busy for 95 % of the period or more goes to
its highest allowed level at once, since it has no idle time to show how
much more it needs. This is the rule of Linux's schedutil governor, and
`scaling_governor` names it so. A CPU held at one level is left alone.
Where the CPU has no APERF and MPERF, the level it was asked for stands
in for the level it ran at.

## Consequences

- These CPUs slow down when idle or lightly loaded, and Intel's reach
  their turbo range under load, rather than staying at the speed the
  firmware set.
- A CPU's speed follows its load within one or two ticks, 10 to 20 ms;
  the CPU's own control, where it has one, is still preferred, since it
  follows the load far faster.
- Each tick of such a CPU reads two counters and, when the state changes,
  writes one register; CPUs with HWP or CPPC, and machines that have
  neither control nor states, do nothing more at their ticks.
- `_PCT` with I/O ports instead of the processor's registers, `_PSD`'s
  software coordination of the CPUs of one domain (the kernel asks each
  CPU for its own state, which hardware coordination resolves), a `_PPC`
  that changes while the system runs, AMD's Core Performance Boost under
  `cpu.boost`, and CPPC through `_CPC`'s shared memory wait in the
  deferred list for a machine that needs them.
- QEMU offers none of this, so the boot tests cannot exercise it; the
  lab's R740, with its firmware's power management handed to the
  operating system and HWP off, is the machine that proves it.
