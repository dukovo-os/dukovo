<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0483. Kernel crash dumps are small text dumps kept in RAM across the restart and saved at the next boot

- Status: Accepted
- Date: 2026-11-13

## Context

The crash log (ADR 0012) keeps what the consoles printed, so that the
next boot can say how the previous one ended and show its last lines.
For a laptop that is the evidence a panic leaves. For a server that
restarts itself at three in the morning (`panic=10`, as the boot menu
gives it), it is not enough: the log says what the kernel last said,
not what it was doing. Which threads were running, what each CPU held,
which locks were taken, how much memory was free and what was on each
stack are gone with the restart, and the person who looks at the
machine the next day has the report's backtrace and nothing else.

Writing that state to a disk from a kernel that has just failed means
trusting the code that failed: the block layer, the filesystem and the
disk driver, with the locks the panic took by force. A dump of all
memory is large, ties up a disk the size of RAM, and is read only by a
debugger with the kernel's symbols at hand. Neither suits a machine
that has to be run from afar by someone with a shell and no debugger.

## Decision

- **A panic writes a text dump into RAM that the restart keeps.** The
  crash log's region grows from 1 MiB to 2 MiB, placed as before (the
  first 2 MiB boundary at or above 4 GiB where there is RAM, else the
  first one from 64 MiB, clear of the kernel's other reservations). Its
  first MiB is the crash log, whose format does not change, so a log
  that an older kernel wrote still reads. Its second MiB is two dump
  slots of 512 KiB, slot *i* for the boot whose log is in log slot *i*.
  A slot is a 64-byte header (`RADIXDMP`, the format version, the
  boot's number, the time of day of the panic in nanoseconds since 1970
  or 0 when the kernel did not know it, the text's length, its CRC-32,
  and whether the dump is whole or cut short) followed by the text. A
  boot clears its own dump slot before anything can panic.
- **The dump is text, as FreeBSD's textdump(4) is.** It starts with
  three lines (`Radix crash dump`, the version, architecture and boot
  number, and the time of day and the time since boot of the panic),
  then the sections `== Panic` (the report as the consoles showed it),
  `== CPUs`, `== Threads`, `== Run queues`, `== Locks`, `== Memory` and
  a `== Stack of CPU N` for the panicking CPU and for each of up to 8
  other stopped CPUs: a word a line, with its address, its value and the
  function it is in when it is an address in the kernel's code. The
  panicking CPU's stack is read from its stack pointer, up to 8 KiB, the
  others' up to 2 KiB each. A person reads it with `less` and attaches
  it to a report as it is.
- **The report goes out first, and the dump is sealed section by
  section.** The panic path writes the report to every console and the
  crash log as before, ends the crash log, and then writes the dump,
  rewriting the slot's header after each section. A panic while the dump
  is written, or a dump that fills its 512 KiB, leaves the sections
  before it, marked cut short; a panic while dumping restarts the
  machine as any panic does, and does not halt it. `crashdump=off` writes
  no dump; `crashlog=off` turns off the log and the dump.
- **The next boot keeps the dump for one boot and says so.** The crash
  log's lines at boot add `Crash log: the previous boot's dump is kept
  for dumps(8) (N KiB)`, with `, cut short` when it is. The dump is
  `/devices/crashdump`, a character device of root's, mode 0600, whose
  `fstat` size is its length: the dump, a line saying that the kernel
  stopped before it was whole when that is so, a blank line and `== Log`,
  and the whole log of the crashed boot, oldest line first. It reads as
  an empty file when the previous boot left no dump.
- **`dumps` saves, lists and deletes the dumps; a service saves at each
  boot.** `dumps save`, which the `dumps` service runs right after logd,
  copies the device to `/system/state/crash/`, named by the time of day
  of the panic and the boot's number, `2026-11-13T10-02-11Z-boot42.txt`
  (a dump from a boot that did not know the time is named by its
  number and a hash of its content, so that saving it twice finds it
  saved). The file is written under another name, synced, renamed, and
  the folder synced, with mode 0600 in a folder of mode 0700. It keeps
  the ten newest dumps within 64 MiB and deletes the oldest past either,
  and logs at warning level the panic's message and the dump's file.
  `dumps list`, `show`, `path` and `delete` work on the saved files, and
  only root runs them. A dump is never sent anywhere.
- **`dumps test` panics the machine on purpose**, after asking at the
  terminal for `panic` to be typed (or with `--yes`), by writing
  `panic` and a line end to the device, which only root can open. The
  kernel logs a warning naming the process and panics with the message
  `dumps test: a panic asked for on purpose`. It checks the whole chain
  once: the report on the console and the serial ports, the restart, the
  dump kept, saved and listed.

## Consequences

- A server that panicked leaves, at its next start, a file that an
  administrator reads with `dumps show`, with the state of every CPU and
  thread and the log of the boot that crashed, and a warning in the log
  that says it is there.
- The dump costs 1 MiB of RAM, on top of the log's, on every machine,
  and is on by default; `crashdump=off` gives it back to the machine
  that wants none.
- A cold power cycle and a firmware that clears RAM at reset lose the
  dump, as they lose the log (ADR 0012). The report still goes out on
  the serial ports and Serial-over-LAN (ADR 0480), so a console server
  that records them has what the report says; the rest of the dump is
  lost with the RAM.
- A dump holds what the kernel held, including the contents of stacks,
  which can hold secrets. Only root reads the device and the saved files,
  the folder is root's alone, and nothing sends a dump anywhere; the
  privacy manual page says so.
- The dump is kept in RAM for one boot, and the service saves it in that
  boot; one that is not saved is overwritten at the boot after.
- `crash-log-uefi` and `crash-log-bios` expect the larger region and the
  line about the dump. The boot test `crash-dump` panics the installed
  system's first start with a self-test, and checks the dump, its file,
  the log's warning and that a user who is not root is refused. The lab
  test `hardware/crash-dump` runs `dumps test` on the lab's machines and
  reads the report from the serial cable.
- Whole-memory dumps for the post-mortem tool of a later release, a dump
  written to a disk from the panic path, and a dump sent with a problem
  report are recorded in `docs/deferred.md`.

## Alternatives considered

- **A dump of all of the kernel's memory, as Linux's kdump makes.** It
  needs a second kernel loaded at boot with memory reserved for it, a
  place to write a file as large as RAM, and a debugger with the
  kernel's symbols to read it. It answers questions the text dump
  cannot, and is deferred for the post-mortem tool; the text dump needs
  none of that and is read where it was saved.
- **Writing the dump to a disk from the panic path, as FreeBSD's
  `dumpdev` does.** It survives a power cycle and a firmware that clears
  RAM, but it runs a disk driver in a kernel that has just failed, with
  locks taken by force, and it needs a place on the disk that nothing
  else uses. It is deferred until a machine needs it; polled drivers are
  the way.
- **A persistent store in UEFI variables or ACPI's ERST, as Linux's
  pstore uses.** It depends on the firmware's support, writes flash on
  every panic, and on most machines holds tens of kilobytes at most, which is not the state a dump
  has.
- **Keeping only the crash log.** It has the report and the last lines,
  and is what ADR 0012 decided; it leaves out the state that says why.
