<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0480. Every 16550 is a terminal with Linux's name, and serial-log chooses the log's ports

- Status: Accepted
- Date: 2026-11-13

## Context

Phase 16 makes a server something to run from afar, and its first item
is a serial console from the boot menu to the sign-in on whichever
port the machine has (plan item B1). Until now only COM1 was a
terminal (ADR 0318). A BMC's Serial-over-LAN, on most Dell, HPE,
Lenovo and Supermicro servers, shows as the board's COM2, at I/O port
2F8h on ISA interrupt 3; Intel AMT's is a PCI function; a rack's
console server is often cabled to a PCI card. Those ports carried the
kernel log and `kdb` (ADR 0360) but no program could open them, and
the log went to every port whether or not something else was cabled
there, such as a UPS, whose controller would read the text as
commands.

The log also went to the ports in step, a FIFO's worth to each at a
time, so the slowest port set the pace for all, and a port that
stalled for a moment and came back was sent the backlog that had
waited, which on AMT with no session open is minutes of old text.

Linux names its 16550s `ttyS0` to `ttyS3` for COM1 to COM4, at 3F8h,
2F8h, 3E8h and 2E8h, whether or not each is there (8250's
`nr_uarts`, 4 by default), and those on PCI from `ttyS4`, in the
order the PCI scan finds them. The same port has the same name on
both systems, so a `console=ttyS1,57600n8` from a server's handbook,
a GRUB entry or a BMC's notes works unchanged. Linux's `console=`
takes a port's options after a comma (rate, parity `n`, `o` or `e`,
data bits, `r` for flow control), may be given more than once, and
the last is the primary console, `/dev/console`.

## Decision

- **Every 16550 is a terminal named as on Linux.** The kernel probes
  COM2 to COM4 at their addresses as soon as the command line is read,
  each used only if it answers, and PCI ports get `ttyS4` onwards as
  the scan binds them; a missing COM port's name is not given to
  another. Up to eleven ports besides COM1 are used: COM2 to COM4 and
  eight on PCI. One kernel thread, `serial`, is the master of every
  serial line, COM1's too, where COM1 had a thread of its own
  (`kernel/radix/src/serial/`).
- **`console=ttyS<n>,<options>` sets a port's line from boot**, before
  the log or GRUB's handover have written to it, from Linux's options
  (`radix_tty::line::Line::from_console_options`); without options it
  is 115200n8. A rate above 115200, the most a 16550 runs at from its
  1.8432 MHz clock, or options the kernel does not read are warned
  about at boot, and the port keeps 115200n8. The terminal starts at
  the same line, with CLOCAL, so that a program sees the line the
  flag set.
- **`serial-log=` chooses the ports that carry the kernel log, `kdb`
  and a panic report**: `all`, the default, as before; `none`; or a
  list of names, such as `serial-log=ttyS0,ttyS4`. A port left out is
  sent nothing by the kernel and is not read by `kdb`. The flag is
  read before the first line of the log goes out, so COM1 too can be
  left out.
- **Each log port sends the log at its own pace from its own place**
  in the 64 KiB ring the log's sink fills (`radix_uart::Console`). A
  slow port no longer holds up a fast one, and a port that falls
  behind the ring skips what gave way. A port busy for 100 ms is
  stalled and skipped, as before; once its transmitter is empty again
  it starts from the first line in the newest 2 KiB, about a
  screenful, so that a management console opened on AMT long after
  boot shows what is happening now, not the backlog. `kdb` and a
  panic report still go to every log port at once, the report last.
- **While a program has a port open, the kernel log stays off it**, as
  on COM1, and a panic report and `kdb` still go out on it if it is a
  log port.
- **Typing comes by interrupt where it can.** COM2 and COM4 share ISA
  interrupt 3, which is edge-triggered: its handler reads both until
  neither has an interrupt pending, up to 16 rounds. COM3's interrupt
  4 is COM1's, so COM3 is polled. A PCI function with MSI gets one
  message for its ports; one without is polled, as the kernel routes
  no INTx. A port's receive interrupt, and the 16550's OUT2, which
  gates it onto the bus on a PC, are on only while its terminal is
  open. A polled port is read every 10 ms while open, and so is a
  port whose interrupt has not yet brought a byte; after that it is
  still read every 100 ms, should an interrupt be lost.
- **`/kernel/class/tty/ttyS<n>` shows each port**, as Linux's
  `/sys/class/tty` does: `port` (`0x2F8`), `console` (`Y` when it
  carries the log), and Dukovo's own `kind` (`com2`, `amt-sol`,
  `ax99100`, `qemu`, `pci`), `pci` (the function's address, for a PCI
  port) and `interrupt` (`isa 3`, `msi` or `polled`). The boot log
  names each port the same way, with its line and whether the log is
  on it.
- **serial-console keeps a console on every `console=` port**, each in
  a process of its own that it starts again should it end; the
  primary console, the last `console=`, runs the installer under
  `install` and Rescue under `rescue=`, and only it runs under
  `rescue=`. consoled leaves the installer to the line when the
  primary console is a serial port. An install over any port keeps
  that port as the installed system's console, by its name.

## Consequences

- A server whose BMC's Serial-over-LAN is on COM2 is used from the
  sign-in with `console=ttyS1`, and AMT's with `console=ttyS4`, the
  same flags as on Linux.
- A UPS or a modem on a serial port is safe from the kernel's text
  with `serial-log=` naming only the console's ports; whether a UPS
  service drives it comes later.
- The boot test `serial-ports` boots with COM2 at 57600 baud and
  QEMU's PCI card, signs in on both, checks their names, lines and
  `/kernel/class/tty` entries, checks that the log stays off a port
  that is open, and that COM2, left out by `serial-log=`, is never
  sent the log. `pci-serial` and `install-serial` keep their checks.
- The lab's `hardware/serial-ports` test checks the OptiPlex's AMT
  port and PCI card as terminals, the AMT port taking typing by MSI.
- The serial-console part of the boot menu, GRUB on the ports a
  machine has and the firmware's console redirection (SPCR) read
  automatically, and the installer on whichever port Enter is pressed,
  follow in the next change of Phase 16.
- INTx for PCI ports without MSI, COM3's interrupt beside COM1's,
  MSI-X-only functions and memory-mapped UARTs, such as AMD's
  `AMDI0020`, are recorded in `docs/deferred.md`; until then those
  ports are polled or not used.

## Alternatives considered

- **Sending the log in step, as before.** It is simpler, but a slow
  or stalled port held up the others, and a port's backlog after a
  stall was old text nobody wanted.
- **A thread for each port, as COM1 had.** Twelve threads that each
  sleep and poll cost more than one that carries every line, and the
  log sending needs one place that sees every port's position anyway.
- **Names in Dukovo's own scheme, such as by PCI address.** They
  would be stable across cards, but a `console=` from Linux, a BMC's
  documentation or a GRUB entry would then name the wrong port.
- **Leaving every port in the log, as Linux does with no `console=`
  for it.** Linux sends its log only to `console=` ports; Dukovo has
  sent it to every port since ADR 0360, for the lab's sake, and keeps
  that as the default, with `serial-log=` to narrow it.
