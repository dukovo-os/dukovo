<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Performance profiles

Dukovo runs well on a machine as it is, and most machines need nothing
from this chapter. The kernel has a few settings that trade one good
thing for another, though, such as how long a program runs before the
next gets its turn, and the right choice depends on what the machine is
for. A *profile* is a choice of values for those settings that suits one
kind of machine, with the reason for each. The reference is tune(8); the
limits one service may be held to are in
[Running services](services.md#limiting-what-a-service-uses).

## What the kernel lets you set

Six settings change how the kernel runs. Everyone reads their values in
`/kernel/settings`, a file each, and a profile sets some of them.

| Setting | What it does | Values | At start |
|---|---|---|---|
| `cpu.energy_preference` | How the CPUs trade speed against power | `firmware`, `performance`, `balance_performance`, `balance_power`, `power` | `firmware` |
| `cpu.boost` | Whether the CPUs may run above their base frequency, in the turbo levels the hardware offers | `on`, `off` | `on` |
| `cpu.speed` | Whether each CPU picks its own speed within its range, or is held at the top of it | `chosen`, `highest` | `chosen` |
| `sched.slice_us` | How long a thread runs before another gets a turn, in microseconds: longer means fewer switches and a longer wait for each | 500 to 20000 | 3000 |
| `disk.read_ahead_kib` | How far ahead a file read in order is read, in KiB: a long run keeps a disk busy, at the cost of memory | 0 to 1024, in steps of 4 | 128 |
| `net.congestion` | How new TCP connections slow down when the network is busy | `cubic`, `reno` | `cubic` |

The energy preference, the boost and the speed are for CPUs that have a
hardware power control, as most recent laptops, desktops and servers do.
With `cpu.boost` at `off` the top of a CPU's range is its base frequency,
and with `cpu.speed` at `highest` the CPU is held there, so its clock
neither falls when it idles nor has to climb when work comes. Where the
firmware keeps the CPUs' speed, as it does on a virtual machine such as
QEMU, there is nothing to set: tune says so in one line and sets the
rest, and `on` and `chosen` are the only values such a machine has.

## The profiles

`tune list` names them, and `tune show NAME` says what one sets and why:

```
$ tune list
balanced     The default: every setting at its start value  (in use)
benchmark    Steady timings: CPUs at their base frequency
low-latency  Quick answers: short slices, CPUs at speed
power-saver  Less power: the CPUs lean to saving it
throughput   Most work done a second, for servers and builds
vm-guest     A virtual machine, whose host sets the CPUs' speed
vm-host      A machine that runs virtual machines
$ tune show throughput
throughput: Most work done a second, for servers and builds
  cpu.energy_preference  performance  The CPUs lean to speed, not to saving power.
  disk.read_ahead_kib    1024         Reads further ahead, which keeps a disk busy with long runs.
  sched.slice_us         6000         Fewer thread switches: more of the time is work, and turns come later.
Every other setting is at its start value.
```

- **balanced** is what a machine runs until someone chooses another. It
  sets nothing: every setting is at the value the kernel started with.
- **benchmark** is for a machine whose timings are compared with
  another's: the CPUs are held at their base frequency, with no turbo,
  so a row measures the code and not the clock it happened to run at
  (ADR 0557). The lab's performance tests choose it, and the Linux they
  are compared with is held the same way.
- **throughput** is for a server or a build machine whose work is long
  runs of computing and reading.
- **low-latency** is for a machine people work at, or whose programs
  must answer quickly: shorter slices and CPUs at speed.
- **power-saver** is for a laptop on its battery, or a machine that is
  mostly idle.
- **vm-host** is for a machine whose work is virtual machines, whose
  virtual CPUs are its threads.
- **vm-guest** is for a virtual machine: its host sets the CPUs' speed,
  so it leaves the energy preference alone.

A profile sets only the settings it lists. Every other setting goes back
to the value the kernel started with, so a profile changes nothing it
does not name, and moving from one profile to another leaves nothing of
the first behind.

## Choosing one

`tune` alone shows the profile in use and each setting's value, with a
`*` before those the profile set:

```
$ tune
Profile: balanced, the default; The default: every setting at its start value
  cpu.boost              on
  cpu.energy_preference  firmware
  cpu.speed              chosen
  disk.read_ahead_kib    128
  net.congestion         cubic
  sched.slice_us         3000

* set by the profile
```

`tune recommend` says which profile fits this machine: `vm-guest` in a
virtual machine, `power-saver` while a battery discharges, and
`balanced` otherwise. Choosing one needs an administrator:

```
$ elevate tune set throughput
the profile is throughput
```

The choice is kept in `/system/settings/tune.toml`, and the `tune`
service gives it to the kernel at each boot, as the kernel forgets
every setting when the machine stops. It applies the choice again when
you make one, which is why `tune set` returns only when that is done. A
setting this machine cannot have is said in one line, and the rest of
the profile is set; `tune` then shows the profile's value without a `*`
and says it is not in effect:

```
$ elevate tune set throughput
the profile is throughput
cpu.energy_preference: this machine cannot have performance; it stays firmware
$ tune
Profile: throughput, chosen by an administrator; Most work done a second, for servers and builds
  cpu.boost              on
  cpu.energy_preference  firmware  (the profile asks for performance; not in effect)
  cpu.speed              chosen
* disk.read_ahead_kib    1024
  net.congestion         cubic
* sched.slice_us         6000
```

`elevate tune set balanced` puts every setting back to the value it
started with.

## A virtual machine

The first time a virtual machine starts after an install, Dukovo leaves
one notice for the sign-in screens, which say that `elevate tune set
vm-guest` suits the machine. It is given once and not again; choosing
any profile, `balanced` among them, ends it. Nothing is changed unless
you choose.

## Seeing what a change did

Each change of a setting is logged by the kernel with the process that
made it:

```
Kernel settings: `tune` set sched.slice_us to 6000
```

and `log tune` has what the service said it set, or could not. Whether
the machine is faster is for you to measure: a profile holds the values
that suit a kind of machine in general, not yours.
