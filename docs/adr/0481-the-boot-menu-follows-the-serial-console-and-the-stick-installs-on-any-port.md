<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0481. The boot menu follows the serial console, the firmware's console port is a console, and the stick installs on any port

- Status: Accepted
- Date: 2026-11-13

## Context

ADR 0480 made every 16550 a terminal named as on Linux and let
`console=` give a port a sign-in, so that a server managed over a
BMC's Serial-over-LAN works from the sign-in on. Three things between
the power button and the sign-in still assume COM1.

- **GRUB's menu is on COM1 only**, at 115200 baud, on the installed
  system and on the install stick. A server whose BMC puts its
  console on COM2 at 57600 baud shows the kernel's log and the
  sign-in there, but the boot menu, and with it the Rescue entry, is
  out of reach. A system whose console is a PCI card's port, or
  Intel AMT's, has no menu on it either.
- **A BMC's console is used by nothing on its own.** UEFI servers and
  most BIOS ones with console redirection describe the port they
  redirect to in the ACPI table SPCR: its address, and the speed it
  was set to. Linux takes that port as a console on Arm servers when
  no `console=` is given. Dukovo read the table for nothing.
- **The stick's serial entry is only for COM1.** *Install Dukovo
  (serial console)* gives `console=ttyS0`. A server whose BMC is on
  COM2 shows the stick's menu on neither, and the person has to know,
  and type, which port to use.

An installed system keeps the port it was installed over by the
`console=` flag the installer writes into the boot menu's
`dukovo_console`. That is a flag and nothing else: the menu cannot be
shown on a port that has no flag, a speed the installer did not choose
is lost, and nothing changes it afterwards short of editing a file on
the EFI system partition that the installer and Repair write again.

## Decision

- **The serial console's settings are a GRUB environment block,
  `boot/grub/serial.env`, on each EFI system partition**, beside the
  menu (`slots::serial`, `slots::envblock`). It holds `dukovo_console`,
  the kernel flags as the menu puts them on each kernel line, and
  `serial_0` to `serial_3` with `serial_0_speed` to `serial_3_speed`,
  the ports the menu shows on, in GRUB's names, and their speeds. The
  flags are `console=ttyS<n>,<options>` for each console, `spcr=off`
  for none, and `serial-log=` for the log's ports. An environment
  block is a file GRUB reads with `load_env` and a program writes
  without GRUB's own tools, and the menu already keeps one beside it
  for the slot state.
- **`serial` writes the block.** serial(8) is a new systools command:
  `serial` lists the serial ports with what each does at this start
  and what the next start will do; `serial console` names the ports
  that have a console, each with Linux's options, or `auto`, the
  firmware's console port, or `off`; `serial log` names the ports that
  carry the kernel log. It writes the block on every EFI system
  partition of the disks the system started from, under sysupdate's
  lock, as the health check does. The menu shows on the console ports
  that GRUB can name at 8 data bits and no parity, up to four, each at
  the console's speed; the output says which console is left out of
  the menu and why. A change acts at the next start, since the kernel
  and serial-console read the command line, which a start makes.
- **The installed menu reads the block.** Without it, as on a system
  installed before it, the menu shows on COM1 at 115200 baud and the
  flags are the `@CONSOLE@` the installer filled in, as before. With
  it, the block's flags take the place of `@CONSOLE@` and the menu
  shows on its ports. Each slot's `startup/menu.cfg` puts
  `${dukovo_console}` on its kernel lines already, so slots from
  older releases follow the block too. The themed menu, which
  replaces GRUB's outputs, appends the serial terminals again.
- **The installer writes the block.** After GRUB and the menu are on an
  EFI system partition, the installer runs `serial console ... --esp`
  with the port it ran on and the line the port ran at, read from its
  terminal, so that the installed system's console and menu are on the
  port at the speed the person used. An install on a screen writes
  `auto`, which is the menu on COM1 and whatever the firmware's
  console is. Repair of startup writes the block only where the
  partition has none, so that what a system had is kept.
- **The firmware's console is a console when no `console=` is given.**
  The kernel reads the SPCR (`radix_acpi::Spcr`): a 16550 in I/O space
  is a port the kernel runs at the table's speed, 8N1, from boot, and
  `/kernel/class/tty/ttyS<n>/firmware_console` holds its line. A
  speed the table leaves as the firmware set it is taken as 115200
  baud, as GRUB takes it. Memory-mapped UARTs and other interface
  types are logged and not driven. serial-console keeps a console on
  that port, never the primary console, so the installer and Rescue
  stay on tty1 unless a `console=` says otherwise. Any `console=` flag
  at all, a screen's too, and `spcr=off` keep the rule from applying,
  as Linux's `console=` does for the Arm servers' SPCR console, so
  that a person's flags are the whole answer.
- **The stick's menu shows on every port a computer is managed from**,
  at 115200 baud: COM1 to COM4 and Intel AMT's Serial-over-LAN
  function, which Intel boards that have it put at PCI address
  00:16.3. A last `serial auto` runs the SPCR's port at the SPCR's
  speed, so that a BMC at another speed wins, and with no SPCR runs
  COM1 again at 115200. A port the computer lacks is passed over. GRUB
  runs a PCI function's port by its address, so another card's port
  has the kernel's console but not the stick's menu.
- **The stick's serial entry offers the installer on every port.**
  *Install Dukovo (serial console)* starts the live system with
  `install serial-console=any` and no `console=`. serial-console keeps
  a console on every serial port, at the firmware's line for the
  firmware's port and 115200n8 for the others, and each shows `Press
  Enter to install Dukovo on this port (ttyS1).` The first port Enter
  is pressed on creates `/system/live/serial-install` with
  `O_EXCL`, in memory, and runs the installer's screens; each other
  port says `The installer is running on ttyS0.` and goes on to root's
  shell. consoled leaves the installer to the line when
  `serial-console=any` is given. A restart frees the claim.

## Consequences

- A server whose BMC is on COM2 shows the stick's menu there at the
  SPCR's speed, and its console after it, with no flag typed. Pressing
  Enter in the Serial-over-LAN window installs there; the installed
  system keeps the port, at that line, in its menu and its console.
- An owner changes where the console and the menu are, and what is
  logged, with `serial`, with no editing of the EFI system partition.
  The menu is out of reach until the next start; `serial` says so.
- Systems installed before the block start as before: GRUB on COM1,
  the console the installer's `console=ttyS0`, if any. Their first
  `serial console` writes the block.
- A port that carries a console but cannot be on the menu, a PCI
  function's second port, a speed with a parity or 7 data bits, a fifth
  port, or a port on another PCI domain, keeps its console; `serial`
  names it.
- The stick shows the menu on up to six terminals, and the installer
  offers itself on every serial port: a UPS or a modem on a serial
  port sees the offer's text there. The entry is chosen by a person who
  wants an install over a serial port, and `serial-console=any` is
  given by that entry only.
- The boot tests `serial-firmware` and `install-serial` cover the SPCR
  port and the installer on the second of two ports, and the block as
  GRUB reads it. The lab's `console/serial-command` test changes the
  settings on the campaign's machine and puts them back.
- Recorded in `docs/deferred.md`: a change acts at the next start only;
  GRUB shows its menu only on a PCI function's first port, only at 8
  data bits and no parity, on at most four ports and in PCI domain
  0000; and the SPCR's "as the firmware left it" speed is taken as
  115200 baud rather than read from the UART's divisor.

## Alternatives considered

- **Editing `grub.cfg` itself.** The installer and Repair write it
  again, and an update replaces the slot's files; a setting in it
  would be lost at the next of those. The block sits beside it and
  neither writes it again unasked, except where Repair finds none.
- **`console=` in `grubenv`.** GRUB writes `grubenv` itself, for the
  saved entry and the counters, and a menu that rewrites it would put
  a person's settings in the way of those.
- **Only COM1 on the stick.** A BMC's port is usually COM2, so the
  stick would still need a flag typed at a menu the person cannot see.
- **Asking which port in the installer's first screen.** The screens
  are on one port already; the question would be asked there, by a
  program that cannot be reached from the others. Enter on the port
  the person is on needs no knowledge of the board's numbering.
- **Reading the UART's divisor for the SPCR's "as the firmware left
  it" speed.** It would be exact where the table says nothing, but
  GRUB takes the same case as 115200 baud, and the menu and the
  console must agree.
