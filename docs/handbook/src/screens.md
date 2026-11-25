<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Screens

Dukovo draws through graphics cards, each with one or more screens: a
screen is a socket a monitor plugs into. A real computer has one card
in release 1.1, `firmware0`, the picture its firmware set up at
start, with one screen. A virtual machine with a virtio graphics card
has `virtio0`, with up to 16 screens that its host plugs in, resizes
and pulls out while the system runs. Drivers for AMD, Intel and
NVIDIA cards, and with them several monitors on a real computer, come
in release 1.10. The full references are hardware(8), consoled(8),
radix-display(2) and ADRs 0700 to 0705.

## Listing the screens

```
# hardware screens
In front: consoled (process 41)

virtio0: Virtio graphics card
  Screen 1: QEMU Monitor (RHT), 260 by 160 mm
    Shows:  1024x768 at 60 Hz
    Modes:  1024x768 at 74.99 Hz (preferred), 800x600 at 60 Hz
    Colour: 8 bits a colour; HDR: no; variable refresh: no
  Screen 2: nothing plugged in
```

The first line says which program has the screens: one program at a
time decides what they show. It is the console, as here; the kernel's
log, before the console has started; or the last picture of a program
that ended, which stays until another takes its place. Then comes
each card with its screens: the monitor by its name, maker and size,
the mode it shows, the modes it offers, and what its colours can do.
A card that says `in the mode the firmware set` cannot change its
screen's mode. The command asks no service, so it works with no
console running, and it needs no rights. No monitor's serial number
is read. `hardware screens --json` prints the same as one line of
JSON, and the web console's Hardware page shows it under
[Screens](web-console.md#screens).

## The consoles on several screens

The console you are on shows on every screen that has a monitor. Each
screen shows the same columns and rows, as many as the smallest of
them has room for; a screen with room for more shows them in the
middle, in the largest text that still shows them all. Plug a monitor
in and the console is on it at once; pull one out, or resize a
virtual machine's window, and the consoles take their new size as a
terminal does when its window is resized. With no monitor at all the
consoles keep running unseen, and show when one is plugged in.

## Several screens on a virtual machine

Give the virtual machine a virtio graphics card with as many screens
as you want. With QEMU:

```
qemu-system-x86_64 ... -device virtio-gpu-pci,max_outputs=4
```

The first screen has a monitor from the start. The others get one
when the host opens a window on them, and take the size the window is
dragged to. With QEMU's `virtio-vga` in the place of the VGA card,
the firmware's picture and the virtio card's are the same screen, and
Dukovo lists `virtio0` alone.

A machine whose only card is `virtio-gpu-pci` has no picture of the
firmware's for Dukovo to draw on: the kernel log starts on the screen
when the card is found, a fraction of a second into the start, and
says `Framebuffer: none` in its first lines.

## When a screen shows the wrong thing

- **Text in the wrong rows, or torn across the screen, on a virtual
  machine.** The host reads the card's memory differently from
  QEMU. Start with `virtio-gpu=store` on the kernel's command line
  (dukovo-boot(7)): text then costs the memory of a whole screen and
  is drawn right on any host. Please report the host.
- **A screen whose `Shows:` line says the card stopped.** The card
  did not confirm a change, and the kernel no longer draws on that
  screen; `dmesg | grep Display:` as root says what it asked. The
  other screens keep working.
- **Nothing on a monitor that is plugged in.** If its `Shows:` line
  says `nothing`, the program in front has not put a picture on it
  yet. The consoles do so within a moment of a monitor being plugged
  in.
