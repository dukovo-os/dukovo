<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0706. Each input device is a file of events in Linux's codes, and the merged files stay

- Status: Accepted
- Date: 2026-11-27

## Context

Until release 1.1 the kernel had two input files. `/devices/kbd` was
every keyboard as one and `/devices/mouse` every pointer as one (ADR
0097, ADR 0148). That is what a text console needs, and it is all
consoled reads.

A window server and a game need more, and none of it could be had
from the merged files:

- Which device an event came from. Two mice were one pointer, a
  tablet could not be told from a mouse, and nothing said what a
  device was called or where it was plugged in.
- When it happened. The time of an event was taken when the
  controller's thread got to run, which is later than the interrupt
  by however long the thread waited.
- A wheel finer than a notch, and a mouse's own counts with nothing
  done to them.
- That only the program a person is looking at reads what they type.
  The merged files were readable by every program on the machine.

Linux gives each device a file of 24-byte events, and a session
service takes the files away from one desktop and hands new ones to
the next when the person switches. The codes of its events are what
key maps, game controller tables and programs made for Linux expect,
and the tree already used them for keys.

## Decision

- **Each device is `/devices/input/N`.** A keyboard, a mouse, a
  tablet, a touchpad: each interface that reports keys or a pointer
  is one, wherever it sits. PS/2, USB on every controller, HID over
  I2C, Hyper-V's keyboard and virtio's devices all add theirs the
  same way. A number is never used twice, so an old file can never
  mean a new device.
- **An event is 16 bytes:** the time in nanoseconds, a kind, a code
  and a value, with Linux's kinds and codes. What one report of the
  device said is followed by a mark, SYN_REPORT, and is put in a
  reader's ring whole or not at all.
- **The time is the interrupt's.** The interrupt handler notes the
  clock and wakes the driver's thread, and every event of the report
  carries that time. Drivers hand reports over on a thread, never in
  the handler, because waking a reader takes a lock that interrupt
  handlers must not take.
- **Motion is the device's counts.** No speed curve and no scaling
  at this level, ever. A wheel is in 120ths of a notch, so a plain
  wheel says 120. A USB mouse whose wheel can count finer than a
  notch says so in its report descriptor and starts out counting
  notches; both USB host drivers ask it to count finer at set-up,
  and read it finer only if it answers that it does. Where a tablet
  points is scaled to 0 to 65,535 on each axis, and INPUT_AXIS gives
  the range.
- **A device says what it is.** INPUT_INFO, INPUT_NAME, INPUT_PLACE
  and INPUT_SERIAL give its bus, maker's and model's numbers, name,
  the place it is plugged in and its serial number. INPUT_BITS gives
  the keys, axes, switches and lights it has, and INPUT_STATE the
  keys that are down. The kernel decides nothing about what kind of
  device that makes it: inputd does (ADR 0707).
- **Each description of a file is a reader with a ring of its own.**
  It has room for 256 events, 4 KiB, when its first event comes, and
  grows only when it must, to 4,096 events at most. A ring that is
  read empty after it grew gives the memory back. A reader that still
  falls behind loses what waited, reads SYN_DROPPED, and asks
  INPUT_STATE. One slow reader costs no other reader an event, and
  nothing in the kernel waits for it.
- **Input follows the screen.** INPUT_TIE ties a description, once,
  to a holder of the display layer (ADR 0702). Events reach it only
  while that holder is in front. When the holder goes to the back the
  reader reads SYN_PAUSED and then nothing, and takes every key as
  let go; when the holder is in front again it reads SYN_RESUMED and
  asks INPUT_STATE. The file is paused where it is. Nothing is taken
  away and nothing is opened again. A reader tied to a holder that
  ended hears nothing more.
- **A key is one reader's from down to up.** A reader of
  `/devices/kbd` that begins to be heard while a key is down, because
  the front passed to its holder, the secure key's hold ended or the
  file was just opened, reads neither that key's repeats nor its
  release. A reader that stops being heard is let go only of the
  keys it read go down. A key held across a switch therefore never
  types into the program that comes to the front.
- **`/devices/input/control` says which devices there are.**
  INPUT_LIST lists them, and a read returns a record for each device
  that came or went.
- **The merged files stay,** in their old formats, fed by the same
  code. `/devices/kbd` still repeats a held key: a PS/2 keyboard
  repeats its own, and the kernel repeats for a keyboard that does
  not. A device's own file never repeats; a reader that wants repeats
  makes them at the rate its person chose.
- **Every input file is root's alone,** mode 0600: the device files,
  the control file and the merged files, which every program could
  read before. inputd hands a device's file to a program that holds
  the screen, and a service that needs a merged file is started with
  the right to open it.
- **One library, `radix_input`, is the whole of it:** which reader
  reads what, the rings, the pauses, the repeats and the secure key
  (ADR 0708). It knows nothing of the kernel, so every rule is tested
  on the build machine. The kernel adds the lock, the files and what
  wakes a reader. `/kernel/class/input/state` describes the devices
  and their readers, without the serial numbers.

## Alternatives

- **Linux's 24-byte event,** so that its programs read the files
  unchanged. Its time is two machine words whose size differs by
  build, so a view for Linux's programs translates it either way.
  16 bytes is a third less to copy at 8,000 reports a second, and
  holds a time that never wraps.
- **One file for all devices, each event naming its device.** A
  reader that wants one mouse would be woken by every key, and a
  device could not be handed to one program without the others.
- **Taking files away on a switch, as Linux's session service does.**
  Every switch then needs a service to act in time, and a program
  must cope with a file that dies under it. A pause in place needs no
  one to act, and a program that ignores the marks still reads
  nothing it should not.
- **The speed curve in the kernel.** Each program would get the same
  one. It belongs to the reader, from a shared library, where a game
  can leave it out.
- **Removing the merged files.** consoled and the volume keys would
  need inputd to work. A text console must take keys with nothing but
  the kernel under it.

## Consequences

- A program that reads a device's file must hold the screen or be
  root. The `input-follow` boot test passes the front between two
  holders and reads what each one's tied reader got.
- A key's time is its interrupt's, where it was a thread's wait
  later before.
- An idle reader holds no ring at all until its first event, and
  4 KiB after it. A mouse that moves on both axes at 8,000 reports a
  second fills 4,096 events in 170 ms, so a reader may stall for
  150 ms and lose nothing; the hub's tests do exactly that.
- A program that read `/devices/kbd` or `/devices/mouse` without
  being root no longer can. In the tree their readers are consoled,
  audiod and the display tests, which run as root; audiod's service
  file names `/devices/kbd` among the files it may read.
- Game controllers (release 1.26) and touchpads' fingers (release
  1.3) are more codes on the same files, and a device made in
  software is one more caller of the hub.
- A wheel on a touchpad or mouse that speaks HID over I2C is counted
  in notches until release 1.3 asks it for its fine steps too;
  docs/deferred.md lists it.
