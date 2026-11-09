<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0365. In a virtual machine, a thread waiting for its disk polls the virtio queue with the interrupt off

- Status: Accepted; an AHCI disc's reads are polled too, polling and
  learning count from the lane's last completion, and a thread waiting
  for a controller that cannot be polled sleeps at once, since [0434](0434-optical-reads-are-held-to-linux-s-in-ob-bench.md)
- Date: 2026-11-03

## Context

Main's nightly Benchmarks job of 2026-11-03 (AMD EPYC 9V45) failed the
gate on the raw virtio rows: a 4 KiB read took 36.0 µs on Dukovo and on
Linux alike (1.001), a 4 KiB write 38.8 µs on both (1.000), and a
64 KiB read 38.3 µs on both. The night before, on an EPYC 9V74, they
were 0.91 to 0.94. Most of each is QEMU's own time: the CPU that waits
sits idle for 26.5 of the 36.0 µs. What is the guest's is the request's
way to the doorbell and back, plus two exits to the hypervisor that
Linux pays too: the halt of the waiting CPU, and the interrupt that
ends it. In the CI runners' nested virtual machines (ADR 0120) each
exit goes through the outer hypervisor as well.

ADR 0116 had the idle loop stop polling for device interrupts, since an
interrupt reaches a halted virtual CPU sooner than a running one. That
holds while the device raises one. A virtio device can be told to raise
none (virtio 1.2, 2.7.7.1 and 2.7.10): the driver then sees the
completion in the used ring, in memory, with no exit at all, which is
how Linux's `virtio_blk` serves its poll queues (`poll_queues=`,
io_uring's IOPOLL).

## Decision

- A thread about to sleep for its request, in a virtual machine and with
  its CPU to itself (no other thread ready there, no switch pending:
  `sched::cpu_to_itself`), polls the request's lane instead, for at most
  the lane's polling limit, and stops as soon as another thread wants the
  CPU. Only then does it sleep as before (ADR 0156).
- While a lane has pollers, `Controller::poll` keeps the device's
  interrupt off. A virtio queue does so with event indexes by asking
  about an index the device has already passed (so it would interrupt
  only 2^16 completions on), as Linux's `virtqueue_disable_cb` does, and
  without them by `VIRTQ_AVAIL_F_NO_INTERRUPT`. When the last poller
  stops, the next look at the used ring that finds nothing asks for an
  interrupt again and looks once more, so that a sleeper's completion is
  never missed. The count of pollers is read under the lane's lock, and
  a part taken out for recovery is set from it when put back.
- The limit follows Linux's cpuidle-haltpoll rule (`radix_sched::HaltPoll`),
  learnt from every wait in the lane: it starts at 0, grows to 50 µs when
  a wait would have ended within 200 µs, doubles for a wait past it, and
  halves for one past 200 µs (a flush to the host's disk). So a disk
  whose requests take long stops polling for them.
- Other controllers keep the default `poll`, which says no: NVMe's rows
  are 0.74 to 0.82, and turning its interrupts off needs an admin
  command per queue. Outside a virtual machine nothing changes: a
  bare-metal interrupt costs no exit.

## Alternatives

- **Poll in the idle loop, the interrupt on.** ADR 0116 measured it
  slower: the host must stop a running CPU to deliver the interrupt.
- **Poll queues only for programs that ask (io_uring's IOPOLL, Linux's
  `RWF_HIPRI`).** Programs written for Linux do not ask, and pay the two
  exits on every read; polling while the CPU has nothing else to do
  takes time from no one.
- **Leave the rows at 1.00.** The gate fails on every runner where QEMU
  is slower than the guest, as it did here.

## Consequences

- A small transfer to a virtio disk in a virtual machine takes no halt,
  no interrupt and no switch; the waiting CPU spins instead, for up to
  200 µs, while no other thread wants it.
- The bench's events table shows the disk rows' halts and switches gone
  where polling caught the completion.
- A lane's limit is shared by its threads on every CPU; a mix of quick
  reads and slow flushes keeps it between the two.
