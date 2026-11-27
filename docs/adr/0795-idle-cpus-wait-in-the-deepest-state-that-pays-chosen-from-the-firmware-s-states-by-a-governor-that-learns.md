<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0795. Idle CPUs wait in the deepest state that pays, chosen from the firmware's states by a governor that learns

- Status: Accepted
- Date: 2026-11-27

## Context

Until now an idle CPU polled for a moment (ADR 0037, ADR 0551) and then
waited in C1, with MWAIT's hint 0 where the processor had it and HLT
where it did not. C1 stops the core's clock and little else. The deeper
C-states (C3, C6, C7 and so on on Intel's processors, CC6 and package C6
on AMD's) turn the core's voltage off and flush its caches, and let the
package drop to its own deeper states when every core is in one; on a
desktop they save several watts at idle, on a laptop most of what its
battery spends while nothing runs. They also free thermal and power
budget for the cores that are busy: a processor gives its turbo
frequencies by how many cores are active, and a core in C1 counts as
active. ADR 0557 found the OptiPlex 7040's busy core held at its
four-core turbo level, 3.3 GHz, where Linux reaches 3.6.

Intel's processors also promote a C1 request to C1E, a lower voltage
that takes several microseconds to leave, unless IA32_POWER_CTL bit 1
is clear. Firmware usually leaves the promotion on. A C1 that should be
short then is not.

The firmware describes the deeper states in each processor object's
`_CST` (ACPI 6.5, 8.4.1): for each, how to enter it (an MWAIT hint, a read
of an I/O port, or HLT), its type (1 to 3, where type 3 may stop the
local APIC's timer and needs bus-master arbitration), its exit latency
and its power. Many firmwares put `_CST` in a table they load only after
the operating system says, through the processor's `_OSC` or the older
`_PDC`, that it can use MWAIT for them. A state is worth entering only
if the CPU stays in it long enough to win back what entering and leaving
cost, its target residency; the kernel knows when the CPU's next timer
fires but not when an interrupt or another CPU's wake-up ends the period
sooner.

## Decision

**Telling the firmware, then reading `_CST`.** Once uACPI has loaded the
namespace, the kernel finds the processor objects (`Processor`, and
`Device` objects with the `ACPI0007` ID) and maps each to the CPU whose
ACPI processor ID or `_UID` the interrupt controller table gave. It
evaluates every object's `_OSC` with Intel's processor interface
(4077A616-290C-47BE-9EBD-D87058713953, revision 1), saying that it
enters C1 with HLT, each CPU's C-states independently, and, where it
uses MWAIT, C1 and the deeper states with MWAIT hints; `_PDC` with the
same bits where `_OSC` is missing. Only then does it read each object's
`_CST`.

**The states a CPU uses.** Each CPU keeps, after its first state (C1 as
before), the `_CST` states it can enter and still keep time:

- an MWAIT state only if the kernel uses MWAIT and the processor lists
  the hint's C-state and sub-state in CPUID leaf 5;
- an I/O-port state only on a machine that is no virtual one, and, as
  the kernel does no bus-master arbitration, only of type 1 or 2, except
  on AMD's processors from Zen on, whose caches stay coherent in their
  type 3 state;
- any deeper state only where the local APIC timer runs in every C-state
  (ARAT) and the TSC is invariant, since a deeper state may stop both
  and the kernel keeps no other clock running for them.

They are sorted by exit latency and named C2, C3 and on in that order,
up to eight in all. A state's target residency is three times its exit
latency: entering takes about as long as leaving, and the state must
save at least what both cost. Each CPU takes its states on itself; CPUs
with the same states share one table. On Intel's processors, where the
table has an MWAIT state of C1 with a sub-state (C1E), the kernel clears
the C1E promotion, so that C1 is short and C1E is chosen when it pays.

**The governor.** Each CPU's idle loop asks a governor which state to
enter. It knows when the CPU's next timer interrupt is due and how the
CPU's recent idle periods ended. It sorts each period into the bin of
the deepest state whose residency it reached, and counts it as a timeout
if it lasted until its timer, within 20 µs, or as an early wake if
something else ended it. The counts decay by an eighth at each period,
so they speak of the last few dozen periods. To choose, it starts from
the deepest state whose residency the time until the timer reaches; if
more than half of the recent periods ended early in shallower bins, it
steps down to the bin that holds the median of those early wakes. A CPU
that a network card or a partner thread wakes every 50 µs stays in a
state it leaves in time; one that sleeps until its timer goes deep.

**Waking.** A state MWAIT enters ends on a write to the line the CPU
watches, as C1 did, so a waker need not send an interrupt. One entered
through an I/O port does not, so before such a state the CPU asks wakers
for an interrupt, as it did for HLT. After the read, the CPU opens a
window for the interrupt that ended the state.

**Flags and reports.** `idle=halt` keeps every CPU in C1 with HLT and
`idle=nomwait` uses no MWAIT, for C1 or for deeper states; `max_cstate=N`
caps the number of states, C1 included. The log names the states of the
first CPU, with how each is entered and its exit latency. Each CPU's
states, with how often it entered each and how long it stayed, are in
`/kernel/devices/system/cpu/cpuN/cpuidle`, with the files and
descriptions Linux's acpi_idle gives, and `cpuidle/current_driver` is
`acpi_idle` where any CPU has a deeper state.

## Consequences

- Idle desktops and laptops draw less power, and a busy core can reach
  the turbo frequencies of fewer active cores, as under Linux.
- A CPU woken from a deep state takes its exit latency longer to start
  the work, up to a hundred or so microseconds; the governor keeps CPUs
  that are woken often in shallow states, and a timer that fires early
  enough is never slept through.
- Machines whose firmware has no `_CST`, and every virtual machine,
  QEMU's boot tests included, keep C1 alone, as before. The deeper states
  are proven on the lab's OptiPlex (Intel, MWAIT states) and its M75q
  (AMD, I/O-port states).
- Processors without ARAT or an invariant TSC, older than about 2010,
  keep C1 alone; no machine the project supports is one.
- The states come from the firmware alone. Intel's processors whose
  firmware gives no `_CST`, or a poor one, would need the kernel's own
  table of each model's states, as some kernels keep; that waits on a
  machine that needs it.
- Linux's `cpuidle` files cannot yet turn a state off while the system
  runs; the boot flags limit the states instead.
