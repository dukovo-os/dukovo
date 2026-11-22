<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0554. `ob bench` gives each virtual CPU a core of its own

- Status: Accepted
- Date: 2026-11-21

## Context

`ob bench` boots Radix and then Linux under QEMU with KVM, on 2 virtual
CPUs (4 for `--scaling`). The CI runners are virtual machines with 4 CPUs:
2 physical cores of 2 threads each (Intel Xeon 6975P-C). The runner's
Linux schedules QEMU's vCPU threads on those CPUs as it likes, so where
the two vCPUs sat differed between the Radix boot and the Linux boot, and
from one run to the next:

- On two threads of one core, a cache line moves between the vCPUs in tens
  of nanoseconds.
- On two cores, a cache line move measured some 170 ns.

The rows that need both CPUs (the "two CPUs" and "cross CPU" rows) follow
that placement. Linux's round trip through shared memory between two CPUs
ranged from 2.2 to 8.8 µs across runs, and Radix's from 4.2 to 5.1 µs. A
ratio between them then said more about which threads the runner had
given each boot than about either system.

## Decision

- **Each vCPU gets a host CPU of its own, the same one every time.**
  After QEMU starts for each `ob bench` boot (the Radix bench, the POSIX
  bench, Linux, and the `--scaling` boots), `ob` asks QEMU over QMP for the
  host thread of each vCPU (`query-cpus-fast`) and pins vCPU `i`'s thread
  with `taskset -p -c` to the `i`th CPU of the host's order. It does so
  within a moment of QEMU answering, long before the guest's benchmarks
  begin.
- **The host's order puts one thread of each physical core first.** `ob`
  reads the CPUs it may run on (`Cpus_allowed_list` in
  `/proc/self/status`) and each one's `physical_package_id` and `core_id`
  from sysfs. It lists one thread per core, lowest numbers first, then the
  cores' second threads, and so on. On a runner with siblings 0,2 and 1,3
  that is 0, 1, 2, 3; with siblings 0,1 and 2,3 it is 0, 2, 1, 3. Two vCPUs
  therefore land on two cores. The 4 vCPUs of `--scaling` take all 4 CPUs,
  which is also a fixed placement.
- **Pinning is skipped, with one note on stderr, when it cannot be
  done:** the host is not Linux, has fewer than 2 cores or fewer CPUs than
  the machine has vCPUs, or QMP or `taskset` fails. A boot without KVM is
  not pinned. The boot goes on unpinned.
- **Each boot prints the placement**, for example
  `pinned vCPUs 0,1 to host CPUs 0,1 (cores 0,1)`, so that a CI log shows
  it.

## Alternatives

- **Pin both vCPUs to the threads of one core.** The placement would be
  fixed, but real multi-core machines are not like it: two CPUs that talk
  to each other are usually on separate cores, and the shared-memory rows
  would measure a cache that no such machine has.
- **Pin the whole QEMU process with `taskset`.** A mask of two CPUs leaves
  the host kernel free to put both vCPUs on one of them, and QEMU's other
  threads would then compete with the vCPUs for them.
- **Leave it.** The two-CPU rows would go on swinging by several times
  between runs, and the gate on them would pass or fail by the runner's
  luck.

## Consequences

- The two-CPU rows of both systems are measured with the vCPUs on separate
  cores, as a real multi-core machine runs them. On the first pinned runs,
  on AMD EPYC 9V74 runners:
  - Radix's two-CPU rows took 1.7 to 2.1 µs, where unpinned runs on the
    same CPU model had taken 2.3 to 2.7 µs. In one run, the ring row was
    interrupted by the host and took 6.0 µs;
  - Linux's took 12 to 15 µs. Its idle vCPUs halt about twice a round,
    which costs two exits to the host and an interrupt each.

  On Intel Xeon 6975P-C runners, pinned:
  - Radix's two-CPU IPC rows took 3.8 to 5.1 µs with the request word of
    [0555](0555-a-cpus-request-to-switch-is-one-word-in-a-cache-line-of-its-own.md),
    before the wake list of
    [0556](0556-a-thread-woken-for-an-idle-cpu-is-handed-to-it-and-that-cpu-queues-it.md),
    and 2.8 to 3.9 µs with it;
  - Linux's took 6.3 µs for the shared-memory channel and 21 to 22 µs for
    the endpoint, notification and ring rows: its idle vCPUs halt between
    rounds there too.
- On some Intel Xeon 6975P-C runner hosts, pinned, a forked process's
  open and close takes about 70 ns more than a thread's, with the kernel
  before [0555](0555-a-cpus-request-to-switch-is-one-word-in-a-cache-line-of-its-own.md)
  as with the one after
  [0556](0556-a-thread-woken-for-an-idle-cpu-is-handed-to-it-and-that-cpu-queues-it.md),
  so the scaling gate fails its one-process row there (585 against
  Linux's 530 ns); on other hosts of the same CPU it takes 511 to 515 ns.
  The cause is still open, in [the deferred list](../deferred.md).
- QEMU's own threads (the I/O thread, the disk and display emulation) are
  not pinned and may share a CPU with a vCPU on a 4-CPU runner.
- Machines where pinning is skipped measure as before.
