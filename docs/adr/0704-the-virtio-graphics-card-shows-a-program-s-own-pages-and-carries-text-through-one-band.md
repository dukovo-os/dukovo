<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0704. The virtio graphics card shows a program's own pages and carries text through one band

- Status: Accepted
- Date: 2026-11-24

## Context

A virtual machine's screen has been the firmware's framebuffer on its
VGA card: one screen in one mode. QEMU, and the hosts built on it,
also offer the virtio graphics card, virtio 1.2's device type 16
(section 5.7): up to 16 screens whose monitors the host plugs in,
resizes and pulls out while the guest runs, a pointer the host draws,
and pictures the host keeps and copies from the guest's memory when
told. It rides on the transport and queues the block, network and
sound drivers already share (ADR 0321).

The display layer (ADR 0700) needs a second card to be an interface
at all, and CI needs screens it can plug in with nobody there. In
its 2D mode the device is small. Its 3D mode is a different device in
all but name and belongs with the graphics drivers of release 1.10.

The device keeps every picture as a _resource_ with a _backing_:
memory of the guest's that the host copies a rectangle from on
TRANSFER_TO_HOST_2D. A text console has no picture to back one with
(ADR 0701), and a resource the size of a screen for each console
would be the 8 MiB a screen the layer exists to avoid.

## Decision

- **`radix-virtio`'s `gpu` drives the device in its 2D mode,** from
  the specification, with no `unsafe` code, and never waits inside
  the driver: what calls it lends it a way to wait. It is tested
  against a card made in software that keeps resources, copies from
  their backing as a host does, and can be told to refuse, to hold
  its answers, or to cut one short.
- **Each device is a card,** `virtio0` and on, with as many screens
  as the device says and a pipe and two planes for each. The device
  has no pipes: any pipe can feed any screens, so a mirror costs one
  copy. A mode is the size of the rectangle shown. The modes of a
  screen are the size the host asks for, first, then those the
  monitor's data lists, and a change may ask for any size from 32 to
  8,192 pixels a side.
- **A program's picture is a resource whose backing is the picture's
  own pages.** A frame is one trip to the host: every command of a
  change is queued, the rectangles that changed are copied and shown,
  and the host is told once. Nothing is waited for but a change of
  mode, which the device must confirm.
- **Text goes through one band.** A picture the card keeps has no
  memory in the guest. Its rows travel through a store of 64 pages a
  card, 256 KiB at any screen size: a transfer's `offset` says where
  in the backing its rectangle starts, so a store of a few rows can
  fill a resource of any height. The kernel's log and its report
  when it stops are drawn the same way, a row at a time from the
  console's cells. `virtio-gpu=store` on the kernel's command line
  gives each kept picture a whole store of its own instead, for a
  host found to read `offset` differently.
- **The host's own pages,** with `virtio-gpu=shared` on a host that
  offers VIRTIO_GPU_F_RESOURCE_BLOB: the host shows the guest's
  pages and nothing is copied. It is off unless asked for until the
  lab has run it on a host that offers it.
- **The pointer is the device's,** 64 pixels a side, on its own
  queue: a move is one command, sent at once, and never waits behind
  a frame.
- **Monitors come and go.** The device's interrupt for a change of
  its screens wakes the card's thread, which asks what they are now
  and tells the layer: holders read EVENT_SCREEN. What a holder
  showed on a screen that went is left alone, and the holder makes a
  new change. Without MSI-X the thread asks once a second instead.
- **The firmware's card steps aside** (ADR 0705) only when its
  framebuffer lies in a BAR of the device this driver took, which is
  QEMU's `virtio-vga`. Beside a separate VGA card the machine has two
  cards.
- **When the kernel stops,** the card is started again, whatever it
  was in the middle of, and the report is sent with what the card has
  already: no memory is made, nothing sleeps and no interrupt is
  waited for.

## Alternatives

- **A resource for every kept picture, sized to the screen.** It is
  the simplest reading of the specification and what
  `virtio-gpu=store` does. It costs 8 MiB a console at 1920 by 1080
  in a guest whose whole point is to be small.
- **Keep the firmware's framebuffer on virtual machines** and write
  no driver until release 1.10. The interface would be tried by one
  card with one screen, hot plug and several screens would first run
  on real graphics cards, and CI could test neither.
- **The 3D mode as well.** It needs a renderer on both sides and
  nothing in release 1.1 would use it.

## Consequences

- A virtual machine has up to 16 screens a card, at any size the
  host's window is dragged to, and the text consoles show on every
  one of them.
- CI plugs screens in and out by acting as a viewer of each: the
  `display-virtio` boot test runs four, changes three in one step,
  and compares each screen with what was drawn, pixel for pixel.
  Every other boot test keeps its VGA card, so both cards run on
  every pull request.
- The band relies on one reading of the specification's `offset`,
  the one QEMU has. A host that differs shows text at the wrong rows
  until `virtio-gpu=store` is set. The lab runs each host it has.
- A host that never answers a command stops the card, not the
  kernel: the driver resets it after a second and does nothing more
  with it, and its screens are marked failed.
- A picture of more than 16,384 runs of memory may be refused by a
  host, since one command names every run.
