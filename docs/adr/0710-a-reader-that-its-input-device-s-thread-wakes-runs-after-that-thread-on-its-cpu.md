<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0710. A reader that its input device's thread wakes runs after that thread, on its CPU

- Status: Accepted
- Date: 2026-11-27

## Context

The display and input rows' first run on CI's runners, under KVM (ADR
0709), gave a mouse's motion 38.8 µs from its interrupt's time stamp to
its reader on Radix and 10.4 µs on Linux, and the motion that 99 in 100
were faster than 62.5 µs and 35.1 µs. Counted in instructions the same
way is shorter on Radix: 7,236 against 10,618.

A motion's way has two wake-ups on Radix. The interrupt wakes the
virtio driver's thread, which is bound to the CPU the interrupt arrives
at. That thread hands the report to the hub, which wakes the reader.
`wake` puts a thread on the CPU it last ran on if that CPU is idle,
since the thread's memory is in that CPU's caches. The reader had last
run on the other CPU, which had slept for the 5 ms since the motion
before. So every motion sent that CPU an interrupt: the kernel counted
397 interrupts sent for 400 motions, and four switches a motion, two on
each CPU. Under a hypervisor the interrupt is an exit to the host, which
then wakes a virtual CPU's thread, and the halt after it is another
exit. On a machine of its own the CPU comes out of an idle state, which
takes longer the deeper it slept. Linux's reader happened to be on the
interrupt's CPU. Its own rule would have sent it to the idle CPU it last
ran on as well.

The waker here is not any thread. A driver's thread that has handed on
what its interrupt brought has nothing to do but wait for the next
interrupt: the CPU it runs on is awake, and free within microseconds. An
IPC call wakes its receiver on the caller's CPU for the same reason
(`wake_sync`, `docs/design/ipc.md`).

## Decision

- **`sched::lazily_handing_on`** is `lazily` (ADR 0550) for a thread
  that waits for its next event once it has handed this one on. A thread
  it wakes that would go to another CPU, idle, stays on this one if the
  waker is all this CPU runs, the thread may run here and is not
  real-time, and, where a processor's cores differ, this CPU is of the
  fastest. It is queued behind the waker and runs when the waker waits.
  A CPU with more to run keeps nothing: behind other threads the reader
  could wait a slice while a CPU sat idle, so it goes where `wake` puts
  it.
- **The rule sits where a wake-up leaves the waker's CPU for an idle
  one** (`place_woken`), as a flag of the CPU that is set and read with
  interrupts masked. A wake-up that stays, or that goes to a CPU with
  threads to run, does what it did before, instruction for instruction.
  A thread has no new field: its 704 bytes fill eleven cache lines with
  none to spare.
- **It holds however the reader waits**: in a read, in poll(2) or on an
  event queue. Each is woken inside the hub's own wake-up.
- **The caller tells the hub.** `input::frame_then_waits` is `frame` for
  such a thread, and the virtio input driver's thread calls it. The
  hub's other wake-ups (a tie, the front passing, the secure key, a
  device that came or went) are made by threads that go on, and stay as
  they were.
- **The USB and PS/2 drivers still call `frame`.** Their threads serve
  other devices, or go on to other work after a report, and a reader
  queued behind one could wait for it. They follow when a motion is
  measured on a machine of its own (`docs/deferred.md`).
- **`ob bench` counts what the kernel does for each display and input
  row**, as it does for the rows of its other boots: switches, timer
  writes, interrupts sent, halts and polls. A display row is counted
  over 100 operations made after the timed ones, and the motions from
  the reader's first wait to its last read. `ob bench --instructions
  --gate` fails if a motion sends more than 0.1 interrupts to another
  CPU (`EVENT_LIMITS`). A reader woken on another CPU counts only 270
  instructions more, which a budget of a tenth over the count does not
  hold.

## Alternatives

- **Any wake-up from a CPU that an interrupt found idle stays there.**
  The woken thread's memory is in another CPU's caches, and a machine
  with many interrupts would gather its threads on the CPU they arrive
  at. Linux prefers the idle CPU the thread last ran on for those
  reasons. Here the waker is known to wait next, which a wake-up in
  general does not say.
- **A flag in each thread**, set while it hands on. It may be held
  across a sleep, where the CPU's flag may not, and it grew every thread
  by 64 bytes.
- **A second form of each wake-up** the hub's can end in: the wait
  table's, the pollers' and the event queues'. No flag, and three more
  functions on paths every program takes.
- **Binding the benchmark's reader to the interrupt's CPU.** The row
  would measure a placement that no program asks for.

## Consequences

- Counted on the build machine: a motion sends 0.01 interrupts to
  another CPU where it sent 0.99, and takes three switches where it took
  four: its interrupt's CPU goes from idle to the driver's thread, to
  the reader, and back. The middle motion counts 6,966 instructions
  (7,236 before), and its budget is 7,700. The motion that 99 in 100
  were faster than counted 7,568 and 8,044 in two runs (8,314 before):
  it differs from run to run and has no budget.
- The reader stays on the interrupt's CPU from its first motion on. One
  that goes on to work there when the next interrupt comes keeps the
  driver's thread waiting for what is left of its slice at most, as any
  thread there would; the driver's thread runs at nice -5. A reader that
  waits again at once, as a window server does between frames, keeps it
  from nothing.
- A real-time reader is placed as before, by its priority.
- The time the rule saves is CI's to give, under KVM, and a lab
  machine's on hardware of its own.
