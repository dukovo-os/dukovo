<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0557. The CPUs can be held at their base frequency, and the lab holds Dukovo to Linux there

- Status: Accepted
- Date: 2026-11-22

## Context

The lab's performance tests hold Dukovo's POSIX rows to those of a
Linux twin on the same machine. Each system ran the CPUs at whatever
clock its own policy reached, and on the Dell OptiPlex 7040 (an
i5-6500, HWP, 800 to 3600 MHz, base 3200 MHz) that clock moved:

- `posix-bench` sleeps 100 ms before every row, so that one row's work
  does not run into the next. Every row therefore starts on a CPU that
  has slowed down, and rows of less than about 100 ms mostly measure how
  fast the clock climbs back.
- Its `cpu_cycle` row, 100 million additions right after that pause,
  took about 55 ms. It read about 1.7 GHz on Dukovo and between 1.7 and
  2.5 GHz on Linux, so it was a reading of the climb, not of the clock
  the other rows ran at.
- Two runs of the same kit booted the Linux twin twice. `linux-bench`'s
  rows agreed within 3%, while `posix-bench`'s differed by up to two
  times (`poll` 455 ns against 233 ns). Dukovo's rows were steady from
  boot to boot.
- The two systems asked the CPU for different things. Dukovo asked HWP
  for its lowest level, 100 MHz, as the floor; Linux's `intel_pstate`
  asks for the most efficient level (800 MHz here), and `amd-pstate` for
  the lowest nonlinear level. Dukovo's CPUs could not be held at one
  speed, nor kept out of turbo, at all.

A row that is slower on Dukovo because of the clock says nothing about
Dukovo's code, and a row that passes because Linux ran slowly hides
what does.

## Decision

1. **Two kernel settings set the CPUs' speed** (ADR 0476).
   `cpu.boost` (`on` or `off`, `on` at boot) lets the CPUs run above
   their base frequency or not: with it off, the highest level asked for
   is the nominal one. `cpu.speed` (`chosen` or `highest`, `chosen` at
   boot) leaves the CPU to choose its speed between the floor and the
   top, or holds it at the top by asking for the same level as minimum
   and maximum. Passive cooling (ADR 0203) still lowers the top, and a
   held CPU is held at the cooled level. Where the firmware keeps the
   CPUs' speed, as on QEMU, only the boot values can be set, as with
   `cpu.energy_preference`.
2. **The floor is the most efficient level**, as with Linux's drivers:
   HWP's and CPPC's most efficient level when the firmware gives one
   between the lowest and the highest, else the lowest. Cooling below it
   takes the floor down with it. `radix_pstate::Request::at` works out
   every request from the levels and the settings, so any order of
   changes ends with every CPU asked for the latest.
3. **`scaling_governor` says which of the two it is**: `performance`
   when held, `powersave` otherwise, the names Linux's `intel_pstate`
   and `amd-pstate-epp` give the same two policies.
4. **A tune profile, `benchmark`, holds the CPUs at their base
   frequency**: `cpu.boost` off, `cpu.speed` highest, and
   `cpu.energy_preference` performance.
5. **`posix-bench` measures the clock three ways.** `cpu_cycle_cold`
   is ten million additions right after the usual pause: the clock a row
   starts at. `cpu_cycle` runs the additions for 250 ms first and then
   times a hundred million: the clock the rows run at once under way.
   `cpu_cycle_end`, the same as `cpu_cycle`, is the last row of the run:
   whether the clock held.
6. **The Linux twin holds its own clock.** The lab's kit passes
   `linux-bench.clock=base` on the twin's command line. `linux-bench`,
   as the twin's first process, then turns turbo off
   (`intel_pstate/no_turbo`, or `cpufreq/boost` for other drivers), sets
   every policy's governor to `performance`, and prints the frequency it
   holds or why it could not. It prints the CPU frequency and idle
   drivers in any case.
7. **The lab's performance tests run `tune set benchmark`** before
   their bench and `tune set balanced` after, since a test that passes
   shares its boot with the next. Where measuring the twin ends
   Dukovo's boot, as on a real machine, the lab measures the twin first,
   so that the profile is set on the boot the bench uses.
8. **The lab refuses to compare differing clocks.** When both systems
   printed `cpu_cycle`, the step's result begins with both clocks; when
   they differ by more than 3%, the step is an error, not a failure, and
   no row is held to Linux. A step's note keeps Dukovo's `BENCH` lines,
   so a run's bundle has the three clock rows.

## Alternatives

- **Hold both systems at their highest speed.** Turbo's frequency
  depends on how many cores are active and on temperature and power
  limits, and the two systems keep idle cores in different states
  (Dukovo in C1 with the firmware's C1E promotion, Linux in its deepest
  allowed state), so their turbo clocks would differ for reasons other
  than the code under test. The base frequency is the one every core
  can hold.
- **Pin the frequency through MSRs from the lab.** It would need the
  twin's `msr` module and root over the lab's console, and would not
  give Dukovo the control a person tuning a server wants anyway.
- **Divide each row by its system's measured clock.** A row's time is
  not all CPU work: memory, interrupts and devices do not scale with the
  core's clock, so the result would be neither system's real time.

## Consequences

- The lab's performance tests on real machines hold both systems at
  3.2 GHz on the OptiPlex, and a run whose clocks differ says so instead
  of reporting rows that are slower or faster for the clock's sake.
  The default clocks, which a user's machine runs at, are no longer what
  those tests measure; Dukovo's idle states and turbo against Linux's
  are left to a later change, measured with these rows.
- Dukovo's CPUs no longer drop below the most efficient level. Below
  it the voltage no longer falls with the clock, so a slower clock does
  the same work for no less energy, and the clock has further to climb.
  This changes ADR 0202, whose CPUs were asked for every level from the
  lowest.
- `ob bench`, which compares Dukovo with itself under KVM, keeps the
  default clocks: the firmware keeps a virtual CPU's speed.
- A kit made before this change has no `cpu_cycle` settled row on
  either side; the lab then prints no clock line and checks nothing,
  as before.
