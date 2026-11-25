<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0701. A picture is the pages of a memory file, held while it lives, and a card can keep one

- Status: Accepted
- Date: 2026-11-24

## Context

The display layer (ADR 0700) puts pictures on planes. Where a
picture's pixels live decides how many times each pixel is copied on
its way to a screen, how much memory the kernel holds for a screen
nobody is changing, and whose memory it is.

Linux gives each card's driver its own kind of buffer. A program asks
the driver for one, maps it, and hands its number to a plane; a window
drawn in ordinary shared memory cannot go on a plane, so a compositor
copies it into a buffer of its own even when the window covers the
screen. Linux's text console keeps a whole copy of the screen in the
kernel at every size, 8 MiB at 1920 by 1080, whether the console
shows or not.

Until release 1.10 every program on Dukovo draws in software, into
shared memory. A text console never has a whole picture at all: it
has cells, and draws the ones that changed.

## Decision

- **A picture is a part of a memory file.** The program opens a file
  of `/devices/shm`, maps it and draws in it. MAKE_PICTURE names a
  part of it as a picture: an offset, a size, a row length and a
  format. MAKE_BUFFER first sizes a file as a card wants one for a
  given size and format, for a program that has no memory yet. A
  memory file the program already holds, a window's own, is a picture
  in the same way, with no copy.
- **A picture holds its pages while it lives.** Cutting the file short
  takes no page from under a screen, and a held page is never swapped
  out. DROP, or the holder's end, lets them go.
- **The maker pays.** The pages are the memory file's, and count
  against the program that made it as any shared memory does. The
  layer cannot be used to keep memory outside a program's limit.
- **A card can keep a picture.** The program maps nothing: it makes
  the picture with PICTURE_KEPT and sends rows with PUT, several
  rectangles in a call, and the card keeps the pixels wherever it has
  them. This is how a text console and the kernel's log draw. A kept
  picture is FORMAT_XRGB8888, as PUT's rows are. Its pixels are
  promised only while it is on a plane and its holder is in front:
  rows sent before then may be lost, and a holder that comes back to
  the front sends them again, which a console can always do from its
  cells.
- **Formats carry the numbers Linux and Wayland use,** so a window's
  format passes through with no table. The cards of release 1.1 take
  32-bit colour, with see-through for the pointer. The interface
  names 10-bit, half-float and video formats for the cards that
  follow, and the check refuses them until one offers them.
- **Device memory is never read.** The picture in memory is the truth:
  what was under a pointer comes back from the picture, and nothing
  the layer does depends on reading a framebuffer.
- **The kernel keeps no picture of its own while a program is in
  front.** For its log and a panic report it keeps what the card
  needs to draw text, which on virtio-gpu is one band of 256 KiB (ADR
  0704) and on the firmware's framebuffer nothing (ADR 0705).
- **Limits.** A holder has at most 4,096 pictures, and a picture is at
  most 16,384 pixels a side.

## Alternatives

- **Buffers made by each driver, as Linux has.** A driver for real
  hardware needs memory the card can scan out from, and MAKE_BUFFER is
  where it will say how it wants a file laid out. Making that the only
  kind of picture would cost every software-drawn window a copy and
  give each driver its own memory accounting.
- **A copy of each screen in the kernel,** so that the layer could
  redraw any screen by itself. It would be 8 MiB a screen at 1920 by
  1080 that nothing in release 1.1 needs: a holder that comes back
  draws from its own pictures, and a console from its cells.
- **Kept pictures whose rows are never lost.** The card would have to
  hold every kept picture's pixels while it does not show, which is
  the copy of the screen again.

## Consequences

- Copies for each pixel that changed: one on the firmware's
  framebuffer, from the picture to the screen; none in the guest on
  virtio-gpu, where the host copies from the picture's own pages, and
  none at all where the host shows those pages itself. A window that
  covers the screen takes the same path as the window server's own
  frame, where Linux adds a copy.
- At 1920 by 1080 the kernel holds 0.25 MiB a virtio card for text and
  nothing a firmware card, where Linux's console holds 8 MiB a screen.
  A console behind a desktop costs no picture at all.
- A holder with eight pictures on a card of four screens costs the
  kernel 2,192 bytes of its own memory
  (`kernel/lib/display/tests/kept.rs`), besides the pages its pictures
  hold.
- A program that draws into a picture while it shows may be seen
  half-way through: the pages are shared with what scans them out. A
  program that must not tear draws into a second picture and changes
  to it.
- A picture of a file the program later cuts short keeps showing the
  old pages. The program sees the cut; the screen does not.
