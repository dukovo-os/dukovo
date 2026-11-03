<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0360. Serial ports on PCI cards and AMT's Serial-over-LAN port carry the console beside COM1

- Status: Accepted
- Date: 2026-11-03

## Context

The test lab (roadmap item 14f) runs Dukovo on a Dell OptiPlex 7040,
which has no serial port on its board. The kernel log and `kdb` need
one: they are the first thing to read when a test hangs, and the only
thing after a panic. The lab gives the box two: a two-port PCIe RS-232
card built on the ASIX AX99100, and the Serial-over-LAN port of the
box's Intel Active Management Technology (AMT), which the management
engine carries over the network to a management console. Until now the
kernel's only serial console was COM1, at I/O port 3F8h
(`arch/x86_64/src/serial.rs`).

Both show themselves on PCI as 16550-compatible serial controllers:
class 07h, subclass 00h, programming interface 02h (a 16550) to 06h (a
16950), with one port's eight registers at the start of an I/O BAR 0.
The AX99100 has one PCI function per port (vendor 125Bh, device 9100h,
subsystem A000h:1000h for a serial port; its parallel port, SPI and
local bus functions have other subsystems and classes). AMT's port is a
function of the chipset's management engine, called KT; its device ID
changes with each chipset. QEMU's `pci-serial`, `pci-serial-2x` and
`pci-serial-4x` (1B36h:0002h to 0004h) are 16550As in the same shape,
with their ports one after another in BAR 0. Cards from other makers
also say they are 16550s, but put more than one port in a BAR in more
than one way; memory-mapped UARTs space their registers in more than
one way too, which the class does not say.

AMT's port may not send at all while no management console has a
session open, and a card's port may be held up by its far end. The log
must not wait on either. The kernel's log sinks run with the log's lock
held and interrupts off, and a byte at 115200 baud takes 87 µs, so a
port cannot be written to from a sink as COM1 is without stalling every
CPU that logs.

Linux drives these ports with `8250_pci` (GPL-2.0), which was not used.
The registers are those of National Semiconductor's PC16550D data
sheet; the IDs are from ASIX's AX99100 data sheet, QEMU's
documentation of its PCI serial devices and the PCI ID Repository.

## Decision

- `radix-uart` (`kernel/drivers/uart`), a compartment with no unsafe
  code, says from a function's IDs and class whether it is a port it
  drives: the AX99100's serial functions (by class or by subsystem),
  AMT's Serial-over-LAN ports (by Intel device ID, from the ICH8's to
  the Core Ultra's, and the 16550 class), QEMU's cards with their port
  counts, and any other function whose class says 16550, whose first
  port alone is used. It checks that a port answers through its scratch
  register and programs it as COM1 is: 115200 baud (divisor 1 of the
  1.8432 MHz clock), 8 data bits, no parity, one stop bit, FIFOs on,
  interrupts off, DTR and RTS up. Only ports in I/O space are used.
- The PCI scan binds them (`kernel/radix/src/uart.rs`) into one console
  of up to eight ports, which carries what COM1 carries: the kernel
  log, from its start (the log replays what it holds when the first
  port is found), a panic report and `kdb`, which reads what is typed
  on any port. They are not terminals that programs open; that is left
  for later, and so is a choice of which ports carry the log.
- The log's sink only copies the text, with `\n` made `\r\n`, into a
  ring of the last 64 KiB, whose oldest bytes give way. A kernel
  thread, `serial`, sends what waits to every port in step, a FIFO's
  worth (16 bytes) at a time once each port's transmitter is empty, and
  sleeps 1 ms between while bytes wait, and from 10 ms up to 100 ms
  while none have. `kdb` and the panic path send at once: `kdb` waits
  up to 1 s for each write, and a panic up to 2 s for the newest 16 KiB,
  so that the report, which comes last, goes out before a backlog.
  Before the machine powers off or restarts, what waits is sent the
  same way.
- A port that stays busy for 100 ms is stalled: it is skipped, at the
  cost of one status read a chunk, until its transmitter is empty
  again, and the other ports do not wait for it. A port that reads all
  ones is gone and stalls the same way.

## Consequences

- The lab's OptiPlex gets the kernel log and `kdb` on the card's ports,
  through a USB serial cable to the computer beside it, and over the
  network through AMT; the T1650 on its own COM1.
- The boot test `pci-serial` boots with QEMU's two-port card, the first
  port on a socket the test reads: the port gets the log from its first
  line, the card's ports are named in the log, and `kdb` answers a
  command typed on the port.
- On any vPro machine whose AMT has its Serial-over-LAN port turned on,
  the log goes there too, as it would to a COM1. With no session open,
  either the management engine takes the bytes or the port stalls;
  neither slows the log.
- A card whose port carries something else, such as a modem, also gets
  the log until its ports become terminals, as COM1 does now.
- The AX99100's 256-byte FIFO, its higher baud rates and its parallel
  port function are not used.
