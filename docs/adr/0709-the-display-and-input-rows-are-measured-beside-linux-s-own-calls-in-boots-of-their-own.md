<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0709. The display and input rows are measured beside Linux's own calls, in boots of their own

- Status: Accepted
- Date: 2026-11-27

## Context

The display layer's short paths have rows (`display-test cost`, ADR
0700): a change checked, a change left to wait for the beat, a whole
picture and a small rectangle shown at once, and the pointer moved, on
the firmware's framebuffer and on a virtio card. They were counted in
instructions and held to budgets, and had no timing beside Linux: the
machine Linux's benchmarks boot on had no graphics card that its DRM
drivers drive, and no input device. The input devices (ADR 0706) add
the path that matters most to a person at the machine: from a device's
interrupt to the program that reads it.

Linux has a like of most of these. An atomic commit with TEST_ONLY
checks a change, the same commit without it shows another framebuffer,
DIRTYFB names a rectangle as new, MODE_CURSOR moves the pointer, and a
mouse is an event device whose events carry the time the driver's
interrupt handler took them at. Three things about the pinned Linux
kernel (`tools/xtask/src/baseline.rs`) decide how they can be measured:

- Booted with a VESA mode (`vga=0x37a`, 1280 by 800 on QEMU's VGA
  card), it drives the firmware's framebuffer with `simpledrm`, as
  Radix drives it with its firmware card.
- `virtio_gpu` holds a blocking commit, and a DIRTYFB, until the next
  of the 60 frames a second it counts with a timer: 10.9 ms of each
  such call passed while the thread slept. `simpledrm` holds nothing.
- `simpledrm` has no pointer plane. A Linux desktop draws the pointer
  into its own picture there.

## Decision

- **Each system measures the rows in a boot of its own**, on the
  benchmark machine with a virtio graphics card of 1024 by 768 beside
  the VGA card and a virtio mouse. Radix runs `display-test cost`, and
  Linux runs `linux-bench --only display` with the pinned kernel's
  `virtio-gpu` and `virtio_input` modules. The other boots keep the
  machine they had, so no other row and no other budget moves.
- **The pairs** are in `DISPLAY_ROWS` (`tools/xtask/src/bench.rs`):

  | Radix | Linux |
  | --- | --- |
  | `display_check` | `drm_check`: an atomic commit of another framebuffer, with TEST_ONLY |
  | `display_change_firmware`, `display_change_virtio` | `drm_flip_firmware`, `drm_flip_virtio`: that commit made |
  | `display_small_firmware`, `display_small_virtio` | `drm_dirty_firmware`, `drm_dirty_virtio`: DIRTYFB of 64 by 64 pixels |
  | `display_pointer_virtio` | `drm_cursor_virtio`: a MODE_CURSOR move |
  | `input_to_reader`, `input_to_reader_p99` | `evdev_to_reader`, `evdev_to_reader_p99` |

  `display_change_queued` and `display_pointer_firmware` have no like:
  Linux refuses a change while the one before has not shown (EBUSY),
  and `simpledrm` has no pointer.
- **Each program says the size of each card's screen**, and `ob bench`
  fails a boot that does not say 1280 by 800 for the firmware's and
  1024 by 768 for the virtio card's, so a row is never compared across
  two sizes of picture.
- **Linux's display rows are the calling thread's processor time**
  (`CLOCK_THREAD_CPUTIME_ID`), each operation timed by itself with the
  clock's own cost taken off, as Radix times its own. Radix's rows are
  the time that passed, which is never less than the processor time. So
  the wait for `virtio_gpu`'s frame is left out, and whatever Linux does
  for a call outside the calling thread is left out with it: the rows
  lean Linux's way, and a Radix row that is under Linux's is under it.
- **The mouse's motions come from outside the two systems.** Each
  program says when it is ready, and `ob bench` sends 400 motions
  through QEMU's monitor, 5 ms apart, to the virtio mouse. Both drivers
  stamp an event with the monotonic clock in the interrupt that brings
  it, and the reader, which waited in read(2), reads that clock when
  the read returns. The rows are the middle one of the 400 and the one
  that 99 in 100 were faster than. Linux gives the stamp in
  microseconds and Radix in nanoseconds.
- **The rows are reported and not gated** until Radix has first met
  parity on them on CI's runners, as every new row is (ADR 0294).
- **The middle motion has an instruction budget, and the slowest have
  none.** Under instruction counting the way from the stamp to the
  reader is the same instructions every time the machine was idle:
  two counts gave 8,035 for the middle one of the 400 both times, and
  7,236 once the driver's thread was bound (below). It is held to a
  budget as the display rows are (`BUDGETS`). The motion that 99 in
  100 were faster than is one that met a timer or another interrupt on
  its way, 9,121 and 9,146 in those two counts, and is reported only.

## Alternatives

- **The time that passed for Linux's rows too.** Each `virtio_gpu` row
  would read 10.9 ms, the wait for a frame that no card shows.
- **Commits that do not block (NONBLOCK) on Linux.** The call returns
  before the change is made, and a worker makes it: less of Linux's
  work would be in the row, not more.
- **Motions made inside each system**, with `uinput` on Linux and a
  test device on Radix. Neither goes through a device's interrupt and
  its driver, which is the path a real mouse takes.
- **The card and the mouse on the machine of the other boots.** The
  kernel draws its log on a screen it finds, and two more devices share
  the interrupt lines: every other row and budget would move for rows
  that do not need them.
- **Gating the rows at once.** No runner has measured them yet, and a
  gate set from one container's emulated run would be a guess.

## Consequences

- `ob bench` boots each system once more per run, and its reports show
  the display and input rows after the others.
- A pinned Linux kernel whose packages lack `virtio-gpu`,
  `virtio_dma_buf` or `virtio_input` fails `ob bench` when its modules
  are packed, and a QEMU whose VGA BIOS has another mode at 0x17a fails
  the size check.
- `sysapi::linux` gains the DRM requests and the event devices that
  `linux-bench` uses, so `linux-bench` still has no unsafe code of its
  own (ADR 0730).
- Counted in instructions when the rows were added, each system under
  QEMU's instruction counting on the build machine, Radix's every
  instruction between the two clock reads and Linux's those of the
  calling thread:

  | Row | Radix | Linux |
  | --- | --- | --- |
  | A change checked | 6,993 | 8,706 |
  | 1280 by 800 shown, firmware | 809,748 | 1,696,787 |
  | 64 by 64 pixels shown, firmware | 26,084 | 25,102 |
  | 1024 by 768 shown, virtio | 12,653 | 27,992 |
  | 64 by 64 pixels shown, virtio | 11,896 | 27,777 |
  | The pointer moved, virtio | 2,643 | 20,099 |
  | A motion to its reader, the middle one | 7,236 | 10,618 |
  | The one that 99 in 100 were faster than | 8,325 | 14,096 |

  The small change on the firmware's framebuffer is the one row where
  Radix counts more: it copies rows of 256 bytes by registers, which
  takes fewer cycles and more instructions than a `rep movsb` does
  (ADR 0775). The timings are CI's to give.
- The rows' first run with both systems, emulated, showed a motion on
  Radix waking one CPU more than on Linux: the interrupt arrives at the
  boot CPU, and the virtio driver's thread woke on whichever CPU it ran
  on last. The thread is now bound to the boot CPU, as Linux binds an
  interrupt's thread to the interrupt's CPUs. The motion's emulated
  time fell from 231 µs to 160 µs, with Linux at 151 µs and 174 µs in
  the two runs, and its count from 8,035 instructions to 7,236.
- What these rows do not show is held elsewhere: that inputd is not
  woken by a key or a motion and stays under its memory bar is the
  `inputd` boot test's, and a whole frame's time, from a client's
  picture to the screen, comes with the window server.
