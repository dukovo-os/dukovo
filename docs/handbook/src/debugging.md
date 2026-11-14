<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Debugging the kernel

This chapter shows how to find out what went wrong when Radix crashes or
misbehaves, in QEMU or on a real machine. The reference is
`radix-debug(7)`; the kernel flags are in `dukovo-boot(7)`.

## Reading a panic report

When the kernel panics, the screen turns into a report under a red
**RADIX KERNEL PANIC** banner, and the same report goes to the serial port:

```text
Radix panic on CPU 0 at 0.327298 s: Page fault (#PF) at 0xffffffffa031416e while writing 0xdead0000: the page is not mapped
  Radix 0.15.0 on x86_64, boot 1; 1 of 1 other CPUs stopped
Registers:
     rax 00000000dead0000     rbx 0000000000000000     rcx 00000000f6d8320b     rdx 000000001fcbfbfd
     ...
Backtrace:
  #0  0xffffffffa031416e  <radix_arch_x86_64::Arch as radix_hal::Platform>::self_test_fault+0x3e
  #1  0xffffffffa030dedf  radix::selftest::fault+0x5f
  #2  0xffffffffa030e1aa  radix::selftest::run+0xca
  #3  0xffffffffa030d496  radix_kernel_main+0x1476
  #4  0xffffffffa0313221  radix_arch_entry+0x1091
Press a key to restart; the log is kept in memory for the next boot.
```

Read it from the top:

- The first line says what happened. For a CPU exception it names the
  exception, the instruction that caused it and, for a page fault, the
  address that was accessed and why that failed. A Rust panic shows its
  message and the source line on the next line.
- **Registers** (after exceptions only) are the CPU state at the fault.
  `cr2` is the faulting address of a page fault.
- **Backtrace** lists the functions on the stack, innermost first. Frame
  #0 is where the fault happened; each later frame is the function that
  called the one above it.

The kernel runs at a random address on every boot (KASLR), so the same
function has a different address each time; the `Layout:` line near the
top of the boot log says where everything was. The names in the
backtrace are right regardless. Two messages point at memory bugs:
`the kernel stack overflowed` (in a double fault) means a stack ran into
its guard page, and `heap corruption:` means the heap found an object
damaged after it was freed, or freed twice. `radix-debug(7)` explains
both.

Below the report the screen shows the most recent log messages, which often
explain what the kernel was doing. Press a key to restart. To photograph
the screen at leisure, that is all the time you need; to restart
automatically instead, boot with `panic=10` (ten seconds).

## After a crash: the crash log

The kernel keeps a copy of everything it prints in a reserved part of
memory that a restart does not clear. After a panic, a press of a reset
button, or a `reboot` in the debugger, the next boot starts with:

```text
Crash log: boot 2; the previous boot (1) ended with a kernel panic. Its last 40 lines:
  | [    0.298033] selftest page-fault: touching an unmapped address; a page-fault panic follows
  | ...
```

This is the main tool on the Dell Inspiron 15 5570, which has no serial
port: a panic restarts into a boot that shows what led up to it. Switching
the machine off, rather than restarting it, loses the log. That makes a
hang on a laptop without a reset button the hard case, since holding the
power button cuts power; when the screen freezes, photograph it, and try
again with `loglevel=debug` to see more of what happened before.

## After a crash: the crash dump

The crash log says what the kernel last said, not what it was doing. When
the kernel panics it also writes a dump of its state, as text, into the
reserved memory beside the log: the panic's report, the CPUs, every thread
and its state, the run queues, the locks held, memory, and the raw kernel
stack of each CPU. The next boot finds it, and says so after the crash
log's lines:

```text
Crash log: the previous boot's dump is kept for dumps(8) (38 KiB)
```

At that boot the `dumps` service, which Spark starts right after the log's
service, copies the dump to `/system/state/crash/`, with the whole log of
the boot that crashed after a line `== Log`, and puts a warning in the log
that names the panic and the file:

```sh
elevate log -s dumps
```

```text
Nov 13 10:04:02 dukovo dumps: the previous boot ended with a kernel panic: selftest crash-reboot: deliberate panic; the dump is /system/state/crash/2026-11-13T10-02-11Z-boot42.txt (dumps show)
```

Only root reads dumps, since a dump holds what the kernel held, including
what was on its stacks; the files are mode 0600 in a folder of mode 0700.
`dumps` tells anyone else to run it with `elevate`:

```sh
elevate dumps list
elevate dumps show
elevate dumps show > crash.txt
```

`dumps list` shows the saved dumps, newest first, with the time of the
panic, the number of the boot that crashed, the size and the panic's
message. `dumps show` writes the newest, or the one named, through
`$PAGER` on a terminal; `dumps path` says where its file is, and
`dumps delete` removes one or `--all`. A dump is plain text: `less` reads
it and `grep` searches it. Its sections are `== Panic` (the report as the
screen showed it, with the backtrace), `== CPUs`, `== Threads`,
`== Run queues`, `== Locks`, `== Memory`, one `== Stack of CPU N` for the
panicking CPU and each of up to eight others, and `== Log`. The ten newest
dumps are kept, within 64 MiB, and none is sent anywhere. The reportd
service keeps each as a crash, with its `== Panic` section alone, and
`report crash` reports it ("Reporting a problem"); the other sections,
which hold the kernel's stacks, never go into a report: read them before
you copy them into one, as you would a log.

A dump lives in the same memory as the crash log, so a switched-off
machine, or firmware that clears the memory when it restarts, loses it as
it loses the log; the report on a serial port or Serial-over-LAN is then
what the panic left (`panic=10` restarts after ten seconds, as the
installed system's boot menu has it). `crashdump=off` on the kernel
command line writes no dump, and `crashlog=off` neither a log nor a dump
(`dukovo-boot(7)`).

To check on a machine that a panic leaves a dump, panic it on purpose:

```sh
elevate dumps test
```

`dumps test` asks you to type `panic`, has the filesystems write their
changes out, as `sync` does, then panics the kernel, which restarts the
machine. After the restart the dump is listed: `elevate dumps list` shows
`dumps test: a panic asked for on purpose`. `dumps(8)` describes the
command, and `radix-debug(7)` the dump's format.

## The kernel debugger

On QEMU, `kdb` gives a command line on the serial port, which `./ob run`
connects to your terminal:

```sh
./ob run --cmdline kdb
```

After `Boot complete.` the prompt appears:

```text
kdb on CPU 0, boot complete. Type `help` for commands, `continue` to leave.
kdb> cpus
4 CPUs, interrupts by xAPIC, timer at 100 Hz:
  cpu0: APIC ID 0, boot CPU, online, 24 timer interrupts, 0 pings
  ...
kdb> sym 0xffffffffa030dedf
0xffffffffa030dedf is radix::selftest::fault+0x5f
```

`help` lists the commands: the log, trace points, CPUs, the memory map
and the memory manager's state (`mem`, `heap`), the capability store
(`caps`), the threads, run queues, resource domains and lock checker
(`threads`, `sched`, `domains`, `locks`), ACPI tables, backtraces,
registers, memory dumps and symbol lookups.
`kdb` also stops at breakpoints and after a panic. `continue` leaves it.

On a machine without a serial port on its board, a PCI Express serial
card gives it one: the kernel finds 16550-compatible ports on the PCI
bus, such as those of cards built on the ASIX AX99100, and sends them
the kernel log, from the start of boot, and `kdb`, as it does COM1 to
COM4. On a vPro machine, Intel AMT's Serial-over-LAN port works the
same way, over the network to a management console. Each port gets the
log at its own pace, and one that stops taking it, as AMT's does with
no session open, picks up at the last screenful when it starts again.
`serial-log=ttyS0,ttyS4` keeps the log and `kdb` on the ports it names
alone. `uart(4)` describes them.

## The lock checker

Boot with `lockdep` to have the kernel check every lock it takes (debug
builds, `./ob run --debug`, do this by default):

```sh
./ob run --cmdline lockdep
```

A deadlock waiting to happen, such as two locks taken in opposite orders
by different code, is then reported the first time both orders have run,
as error lines starting `Lock checker:` that name where each lock was
created and taken. `radix-debug(7)` explains the reports; `kdb`'s `locks`
command shows what the checker has seen.

## Debugging with GDB

For stepping through code, use QEMU's GDB stub:

```sh
./ob run --gdb --debug        # terminal 1: QEMU waits for GDB
gdb -x out/qemu/radix.gdb     # terminal 2
```

GDB stops at `radix_arch_entry`, the kernel's first Rust function, with the
kernel's symbols loaded. From there `break`, `next`, `step`, `bt` and
`print` work as usual; `--debug` builds the kernel without optimisation so
stepping follows the source. `--gdb` boots the kernel with `nokaslr`, so
it runs at the addresses its symbols name.

## Trace points

Some events, such as interrupts, happen too often to log. Trace points
record them cheaply in a ring of 1024 events. Boot with `trace` to record
from the very start, then type `trace` in `kdb` to see them:

```sh
./ob run --cmdline "kdb trace"
```

## Reporting a crash

When you report a kernel crash in an issue, include:

- the whole panic report (a photo of the screen is fine) and, if the
  machine restarted, the crash log lines from the next boot and, once
  you have read it, the dump (`elevate dumps show > crash.txt`);
- the output of `git rev-parse HEAD` for the source you built;
- the machine, and whether it booted through UEFI or legacy BIOS (the
  `Firmware:` line at the top of the boot log);
- the kernel flags used, from the `Command line:` line.
