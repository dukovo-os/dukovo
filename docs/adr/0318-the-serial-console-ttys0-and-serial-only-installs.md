<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0318. The serial console: ttyS0 and serial-only installs

- Status: Accepted; every serial port is a terminal, carried with COM1's by one `serial` thread, and serial-console keeps one on each `console=` port, since [0480](0480-every-16550-is-a-terminal-with-linux-s-name-and-serial-log-chooses-the-log-s-ports.md)
- Date: 2026-11-08

## Context

Phase 14c's servers install "fully over a serial console", and the
phase's exit asks for a serial-only install to succeed. Many servers
and most virtual machines in a rack have no screen, only a serial port
or a BMC's serial-over-LAN, which shows as the board's COM1. The
install stick's menu, from the plan, has an *Install with serial
console* entry.

Until now COM1 was the kernel's alone: the log went out on it from the
first instruction, GRUB's menu was mirrored on it (ADRs 0307, 0308),
and `kdb` read it, but no program could open it. The installer's
screens and every sign-in ran on consoled's virtual consoles, which
need a screen; consoled exits on a machine without one. USB serial
ports already were terminals whose master is a kernel thread (ADR
0246).

## Decision

- **COM1 is the terminal `/devices/ttyS0`**, as Linux names it, made
  as a USB serial port's terminal is, with a kernel thread, `ttyS0`,
  as its master. Typing comes in by the port's interrupt (ISA IRQ 4),
  which is on only while a program has the terminal open; output goes
  to the transmitter a FIFO's worth at a time. The line's speed and
  character format follow the terminal's settings, through the 16550's
  divisor and line control register; what a USB serial port and COM1
  read from those settings is now the tty crate's (`radix_tty::line`).
  The terminal starts at 115200 baud, 8N1, the line GRUB and the log
  run, with CLOCAL and without HUPCL, so that DTR stays raised for the
  log. A 16550 has no flow control of its own, so CRTSCTS is not
  honoured.
- **A serial line names itself for `ttyname`**: Dukovo's own terminal
  request GET_LINE_NAME answers `ttyS0`, or a USB serial port's
  `ttyUSB0`, where a pseudo-terminal answers GET_PTY_NUMBER, so that
  tty(1), the sign-in's records and the installer know the line.
- **While a program has ttyS0 open, the kernel log stays off the
  line**, as the program draws on it: the log is kept, for `dmesg` and
  logd, and goes out on the line again once the last program closes
  it. A panic report and `kdb` still go out on it; at a breakpoint,
  where the other CPUs keep running, `kdb` turns the receive interrupt
  off so that what is typed is its own.
- **`serial-console`, a systools tool and a service started at every
  boot, keeps the console**, as agetty does on Linux. It acts only
  when the kernel was started with `console=ttyS0`, Linux's flag, with
  Linux's options after a comma (`115200n8`); otherwise it exits with
  0 and stays stopped. It runs the installer's screens once under
  `install`, root's shell under `console-sign-in=off`, and the issue
  text and login otherwise, each again as it ends. Each session gets a
  new terminal's settings, TERM=vt220, as systemd's serial consoles
  have, and the size of the terminal at the other end, which it asks
  as xterm's `resize` does: the cursor sent to the far corner and a
  cursor report asked for, with 80 by 24 when nothing answers within
  half a second.
- **consoled leaves the installer to the serial line** when the
  command line has both `install` and `console=ttyS0` and the machine
  has COM1, so that the screens never run twice; tty1 runs the shell.
- **On the serial line the screens are plain ASCII**, as the approved
  design draws them there: boxes of `+`, `-` and `|`, `(*)` for a
  chosen option, and *(serial console)* after the title. A terminal
  program, and above all a BMC's console, is not sure to show UTF-8.
- **The stick's menu has *Install Dukovo (serial console)***, with
  `live install console=ttyS0 console-sign-in=off`, named as its
  *Try Dukovo (live console)* is, and after it, so that the entries a
  person with a screen uses keep their places.
- **An install over the serial port keeps the serial console.** The
  installed menu's Dukovo and startup-trouble entries have a
  `@CONSOLE@` placeholder, which the installer fills with the live
  system's own `console=ttyS0` word when it runs on ttyS0, and with
  nothing otherwise. The answers file's `serial_console` says it
  outright; the screens save it as they ran, and the review's settings
  line ends with *serial console ttyS0*.

## Consequences

- A computer with no screen installs from the stick and is used over a
  serial cable, from GRUB's menu to the sign-in, with any terminal
  program at the other end.
- The boot test harness has a terminal at the serial port's far end
  (`tools/xtask/src/vt.rs`): it takes in what the port carries,
  answers cursor reports, and shows its rows to the cases.
  `install-serial` installs on a machine with no video card, with
  every page answered by keys on the port, and signs in on the port
  at the installed system's first boot.
- The lab's `hardware/serial-port` test checks COM1's terminal on a
  machine that has one: its speed read and set, and serial-console
  stopped without `console=ttyS0`. The lab types on the keyboard and
  reads its serial console through Intel AMT's Serial-over-LAN, a PCI
  port, so the serial-only install itself is the boot test's.
- Serial ports on PCI cards and AMT's stay the kernel's log and `kdb`
  ports (ADR 0360); programs cannot open them yet.

## Alternatives considered

- **The kernel log interleaved with the console, as on Linux.** Linux
  prints messages above the console log level on `console=ttyS0`,
  which draws over a full-screen program such as the installer. The
  log is kept and readable meanwhile, so it stays off the line.
- **consoled drawing a console on the serial port too.** consoled is
  built round the screen and keyboard, and exits without them; a small
  program for one line is what Linux's getty is.
- **Asking the terminal's type.** Terminals answer a primary device
  attributes query in many ways; vt220 is what nearly all of them
  understand, and a person can set TERM in their shell.
