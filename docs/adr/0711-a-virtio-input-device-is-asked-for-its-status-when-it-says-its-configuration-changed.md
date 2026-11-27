<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0711. A virtio input device is asked for its status when it says its configuration changed

- Status: Accepted
- Date: 2026-11-27

## Context

With ADR 0710 a mouse's motion reaches its reader on Radix in fewer
instructions than on Linux, with fewer switches, fewer timer writes and
no interrupt sent to another CPU. On CI's runners, under KVM, it still
took twice Linux's time: 23.7 µs against 11.8 µs.

What the kernel does not count is what a device's register costs. In a
virtual machine a read of a virtio device's configuration leaves the
machine for the host's emulator and comes back with the answer, which
takes longer than every instruction of the motion's way together. With
each read and write of a device's memory traced in the emulator, a
motion on Radix read one register and Linux's read none. The register
was the mouse's device status.

The read was the driver's thread asking whether the device needs a
reset (DEVICE_NEEDS_RESET), which it did for every device at the end of
every run. Since ADR 0710 the reader of the motion is queued behind that
thread, so it waited for the read; with a keyboard, a mouse and a tablet
it would have waited for three.

A device does not need to be asked. One that sets DEVICE_NEEDS_RESET
after the driver said DRIVER_OK must send a configuration change
notification (virtio 1.2, section 2.1.2), and with MSI-X that
notification has an entry of its own.

## Decision

- **A virtio input device is given two MSI-X entries**: entry 0 for the
  completions of both its queues, as before, and entry 1 for a change of
  its configuration (`radix_virtio::input::Vectors`). The second
  interrupt marks the device, and the thread asks a marked device for
  its status. Taking events reads memory the device wrote and no
  register.
- **A device is asked once when it is set up.** One that failed before
  the driver said DRIVER_OK owes no notification.
- **A device with one MSI-X entry, or one for which no second vector is
  free, raises entry 0 for either**, and is asked at each of its
  interrupts, as every device was. It works as before and costs what it
  did.
- **`ob bench` asks the host what it did for each motion.** Under KVM it
  reads KVM's counters for each system's virtual machine before the
  first motion and after the last (QMP's `query-stats`) and reports them
  for each motion, Radix's beside Linux's: the exits, and of them those
  for device memory, for I/O ports and for a halt. `ob bench --display`
  measures the display and input rows alone, on both systems, in a few
  minutes.

## Alternatives

- **Asking when an interrupt brought no event**, with one entry for
  everything. A device that fails with events still to be read would
  not be asked, and sends nothing after.
- **Never asking**, as Linux's driver does. A device that stopped would
  stay among the input devices and say nothing; here it leaves them,
  and the kernel's log says why.
- **Asking on a timer.** A read for each device at each tick, for as
  long as the system runs, to learn what the device tells by itself.
- **Asking after the reader ran**, by handing the frame on and asking
  in a later run of the thread. The read would still be made for every
  motion, on a CPU the next motion needs.

## Consequences

- Traced in the emulator, 400 motions read a register of the mouse 400
  times before, and not once now.
- Under KVM on the lab's PC the host counted 1.18 exits for device
  memory for each motion before and 0.17 after, against Linux's 0.01:
  what is left is the USB and disk controllers' registers, which the
  trace shows touched a few times a second whether a mouse moves or
  not. A motion took 13.3 µs before and 5.3 µs after, and the motion
  that 99 in 100 were faster than 30.6 µs and 8.3 µs. Linux's took 18.6
  µs and 10.7 µs in the same two runs, and 36.0 µs and 19.5 µs.
- CI's runners are virtual machines themselves, where an exit to the
  emulator costs more than on a machine of its own: their time for a
  motion is the benchmark job's to give.
- An input device takes two of the kernel's interrupt vectors where it
  took one.
- A device that needs a reset leaves the input devices as before, and
  the kernel's log says so.
- The other virtio drivers ask a device for its status only where
  nothing waits behind the answer: the network card when no packet
  moved since the last look, the graphics card and the sound card while
  they wait for an answer. The sound card's worker also asks at each of
  its looks, 50 a second while it plays, which `docs/deferred.md`
  records.
