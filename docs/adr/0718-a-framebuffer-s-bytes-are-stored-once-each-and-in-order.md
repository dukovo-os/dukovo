<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0718. A framebuffer's bytes are stored once each and in order

- Status: Accepted
- Date: 2026-11-27

## Context

The firmware's framebuffer is a card (ADR 0705) whose driver copies a
picture's pixels to the screen's memory, which the kernel maps
write-combining. Under KVM on the lab's PC, an AMD Ryzen 9 9900X3D, a
new picture of 1280 by 800 took 516.9 µs to show where Linux's
`simpledrm` took 187.1 µs, 2.76 times as long, while the same row on
CI's runners took 0.81 times Linux's. AMD's nested paging gives a guest
the write-combining it asks for, as a real machine does. The Intel
hosts CI runs on give a guest's screen ordinary cached memory whatever
the guest asks, so nothing there showed what write-combining costs.

A CPU gathers the stores to write-combining memory a cache line at a
time, in a few buffers, and sends a line on when it is full and the
stores have moved past it. Three things the driver did went against
that, each measured on that PC by copying a picture of 1280 by 800 to
the framebuffer 15 times at boot and taking the fastest:

| How the 4 MB were copied to the framebuffer | Time |
| --- | ---: |
| `rep movsb`, all of it in one copy | 146 µs |
| By registers, 32 bytes a turn, each byte once and in order, a row at a time | 148 µs |
| The same, a row cut where the picture's pages end: 1,800 copies | 148 µs |
| The same, with a locked instruction after every 48 rows | 149 µs |
| after every 12 rows | 152 µs |
| after every 3 rows | 168 µs |
| after every row | 217 µs |
| `rep movsb`, `rep movsl` or `rep movsq`, a row at a time | 217 µs |
| `movnti` by registers with a store fence after each row | 217 µs |
| `rep movsb`, rows cut at the pages | 290 µs |
| The kernel's `memcpy` (ADR 0775), a row at a time | 305 µs |
| The kernel's `memcpy`, rows cut at the pages, as the driver copied | 478 µs |

- **A `rep movs` to such memory ends with a wait.** Each one costs
  87 ns more than its bytes, the same as a store fence after it: the
  CPU sends on what it has gathered and waits until it is sent. A long
  copy pays that once. A picture's pages are wherever the program's
  memory is, so a row is copied in the pieces its pages cut it into,
  2.25 a row.
- **A store that goes back costs more.** `memcpy` stores the first and
  the last 8 bytes of a long copy by registers and the rest by
  `rep movsq`, so the last 8 are stored twice: once by the `rep`, and
  again after their line was sent. The line opens again for 8 bytes,
  which go out on their own. Each copy cost some 190 ns more than its
  bytes.
- **A locked instruction is such a wait too.** The driver holds the
  console's lock while it writes, a few rows a hold, and taking a lock
  is a locked instruction. A hold was 16 KiB of rows, three rows of
  this screen, so a picture took 267 of them.

To ordinary cached memory, the same framebuffer mapped a second time,
every one of these ways took 58 to 62 µs but the copies by registers,
which took 65 to 73 µs, and `movnti`, which took 217 µs.

## Decision

- **The x86-64 kernel has a copy for write-combining memory**,
  `copy::in_order`, beside its `memcpy`: 32 bytes a turn by
  general-purpose registers, four loads and then four stores, and then
  16, 8, 4, 2 and 1 bytes as the bits of what is left say. Every byte
  is stored once and the lowest address first, whatever the length and
  wherever the two ends lie.
- **The HAL offers it as two calls**: `Cpu::copy_in_order`, between two
  slices, and `UserMode::read_frames_in_order`, from RAM by its
  physical address, as `read_frames` is. An architecture that does not
  say otherwise copies both by its plain copy.
- **The firmware's card writes the screen by nothing else.** A row
  taken straight from a picture's pages goes by
  `Pages::read_to_screen`, and a row the pointer was blended into, or
  one PUT sent, by `copy_in_order`. A framebuffer in another layout
  than the pictures' is written a pixel at a time, which is in order
  already.
- **A hold of the console's lock is 64 KiB of rows**, from 16 KiB: 67
  holds for this picture. At 28 GiB a second, which is what that PC's
  memory takes, a hold masks interrupts for some 2 µs, as long as a
  hold of 16 KiB took before.
- **The `copy` self-test** (ADR 0775) has the copy in order make every
  copy it has `memcpy` make, and checks each the same way.

## Alternatives

- **One `rep movsb` for the whole picture.** The fastest way measured,
  by 2 µs, but only for a picture whose pages follow one another in
  physical memory, which a program's rarely do; cut at the pages it is
  twice as slow as the copy by registers.
- **`memcpy` itself without stores that overlap.** The overlap is what
  lets it copy a short run with no branch for each size (ADR 0775), and
  to cached memory it costs nothing. The kernel knows which memory it
  mapped write-combining, and the callers that write there are few.
- **`movnti`.** To write-combining memory an ordinary store already
  behaves as a `movnti` does. The fence it would need after a copy is
  the wait this change takes away.
- **Mapping the framebuffer cached.** Two and a half times as fast
  again under a hypervisor, where the screen is RAM. On a real machine the screen is
  a device's memory: what a cache holds back is not on the screen, and
  each line would be read from the device before it is written.
- **Longer holds, or none.** At 256 KiB the same picture drew 5 µs
  sooner and with the lock held for the whole picture 7 µs sooner, at
  four and sixty times the 2 µs with interrupts masked. A screen whose
  handing over needs no lock in the driver is a larger change to the
  console than this one.

## Consequences

- On that PC under KVM the picture's copy takes 148 µs where it took
  478 µs, and showing a new picture of 1280 by 800 takes 171 µs where
  it took 516.9 µs: 0.91 to 0.92 times Linux's 186 to 187 µs, from 2.76
  times.
- Where a hypervisor gives the screen cached memory, the copy by
  registers is slower than `rep movsq`: 66 to 68 µs against 59 µs for
  this picture on that PC. A real machine's framebuffer is never such
  memory.
- The copy counts more instructions than a `rep` did, three to a
  quadword where QEMU counted one, so the row's budget of instructions
  rises (`ob bench --instructions`).
- The kernel log's console still draws its lines on the screen by
  `memcpy`, a row of a line of text at a time (`docs/deferred.md`).
