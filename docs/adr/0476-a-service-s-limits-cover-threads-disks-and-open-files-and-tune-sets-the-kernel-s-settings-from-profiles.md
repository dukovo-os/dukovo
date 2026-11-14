<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0476. A service's limits cover threads, disks and open files, and tune sets the kernel's settings from profiles

- Status: Accepted
- Date: 2026-11-13

## Context

A service's file could limit its memory and its CPU weight (ADR 0098).
Three things a service can exhaust were not limited, and one thing it
needs was not set:

- Threads. A fork bomb in one service made threads until the machine had
  none, and every other service and every shell stopped answering.
- Disks. Two services reading one disk shared it as the queue's order
  happened to give it, by `ionice` class and level (ADR 0093) and no
  more, so a backup could take a database's disk. Nothing counted what a
  domain read and wrote, so nothing said who was using it.
- Open files. Spark did not set a service's `open_files`
  (RLIMIT_NOFILE), so nginx raised its own limit to the hard limit, 4096
  (ADR 0491).
- Settings. The few kernel values that suit one kind of machine and not
  another, such as the scheduler's slice or the page cache's read-ahead,
  were constants, and a machine whose work was virtual machines or builds
  ran with those of a laptop.

systemd limits these with cgroup v2's `pids.max`, `io.weight` and
`io.max`, but its `IOWeight=` does nothing unless the disk uses the BFQ
scheduler or the `io.cost` controller has been set up for it, and most
SSDs use `mq-deadline` or `none` without it; `tuned`'s profiles are dozens of
lines of sysctl and sysfs writes with `include=` chains, whose switching
back can leave settings behind.

## Decision

1. **A domain limits its threads.** `set_threads(limit)` on a resource
   domain (version 3, radix-resource-domain(2)) limits the threads of the
   domain and those below it together. A thread past it is not made, and
   `fork` and `pthread_create` fail with EAGAIN, as with Linux's
   `pids.max`. Spark gives a service 4096 unless its file says another
   number.
2. **A domain counts its disk use.** Each domain counts the bytes its
   threads read from disks and write to them, and the time its requests
   waited and its rates held them; each process counts its bytes
   (`disk_read_bytes` and `disk_written_bytes` in `/processes/N/status`).
   Reads and direct writes count when the device is done. A write to a
   file counts when it has left pages dirty, so that writeback, which is
   the kernel's own, never counts twice. `usage` and `io` give the
   counters.
3. **A domain has a disk weight and rates.** `set_io` takes a weight, 1
   to 10000 (100 unless set), and rates of bytes and requests read and
   written each second (`NO_LIMIT` for none). While any domain has a
   weight other than the default, each disk lets up to 8 requests go at
   once and serves those that wait by `ionice` rank, then by start-time
   fair queuing over their domains' weights, flattened along the tree.
   A weight orders only the requests that wait, so the depth is kept
   below what the services that share a disk keep on it: at 32, two
   services of 16 threads each had all their requests on the disk and
   shared it evenly whatever their weights.
   With no weight set, the queue is as before. A rate is a bucket of a
   second's worth for the domain across all disks together; a thread
   waits before a request, or once its write has left pages dirty, until
   the slowest rate of its domain and those above it allows it. Because
   the weight is in the one queue every disk uses, it holds whatever the
   disk's own scheduler is.
4. **The kernel's settings are a capability.** `kernel_settings`
   (radix-kernel-settings(2)) lists the kernel's settings, each with its
   value, the value it had when the kernel started and the values it
   takes, and sets one, logging who did. The kernel gives Spark the one
   capability, and Spark gives a copy in `SLOT_SETTINGS` to a service
   whose file says `admin.tune = true`. Everyone reads the values in
   `/kernel/settings`, a file each. The first settings are
   `cpu.energy_preference` (through HWP or CPPC, and not supported where
   the firmware keeps the CPUs' speed, as on QEMU), `sched.slice_us`,
   `disk.read_ahead_kib` and `net.congestion`.
5. **A process's limits are set before it runs.** `start_limit` on a
   process capability that has not started sets a resource limit, and
   raises the hard limit if need be, RLIMIT_NOFILE's to at most 1048576,
   Linux's `fs.nr_open`. The C library reads the file limit when the
   program first needs it, so the program starts with what its service
   file names. Once the process has started the call fails.
6. **Service files say them.** `[limits]` gains `io_weight` (1 to
   10000), `io_read` and `io_write` (bytes a second, written as `memory`
   is), `io_read_ops` and `io_write_ops`, `threads` (1 to 1048576, 4096
   unless set) and `open_files` (1 to 1048576). `service check` refuses a
   number out of range with its line and column, and the bundle service
   writer carries `open_files`. Spark gives the domain the weight, rates
   and thread limit when it makes it and at each start, as it does the
   CPU weight and memory, and sets `open_files` on the child before it
   runs. nginx's file names `open_files = 65536` and its own raising of
   the limit is gone.
7. **Status says what a service is held to and what it used.** `service
   status` gains `Limits:` and `Use:` lines, and Spark's `show NAME`
   gains `usage`: memory now and at its peak, the charges the limit
   refused, threads, bytes read and written, the time its requests waited
   for a disk and the time its rates held them. A service ended by a
   refused charge is logged and shown as *stopped at its memory limit*,
   which is not the out-of-memory policy's stop, as that acts only when
   the machine's memory is gone.
8. **Profiles are data; tune applies them.** A profile is a TOML file in
   `/system/resources/tune`, a description and the settings it sets, with
   a comment above each saying why: `balanced` (sets nothing, the
   default), `throughput`, `low-latency`, `power-saver`, `vm-host` and
   `vm-guest`. `tune` shows the profile in use and each setting's value,
   marking those the profile set; `tune list` and `tune show NAME` say
   what the profiles are and why; `tune recommend` says which fits
   (`vm-guest` under a hypervisor, `power-saver` while a battery
   discharges, else `balanced`). `elevate tune set NAME` writes
   `/system/settings/tune.toml` and restarts the `tune` service, the one
   service whose file says `admin.tune`, which runs `tune apply` at each
   boot and when a profile is chosen. It sets each setting the profile
   lists that is not at its value, and puts every other setting back to
   the value the kernel started with, so a profile changes nothing it
   does not list and switching leaves nothing of the one before. The
   service's file says `ready.when = "ended"`, so the restart waits for
   it. A setting this machine cannot have is one line and no failure; a
   value the kernel refuses, or a setting this kernel lacks, ends the
   service with a failure, and `tune set` fails with it.
9. **A virtual machine is told once.** The first time the service runs
   in a virtual machine (the hypervisor bit of CPUID) with no profile
   chosen, it leaves a notice for the sign-in screens (ADR 0504): "This
   machine runs in a virtual machine: `elevate tune set vm-guest` suits
   it." A mark in `/system/state/tune` keeps it from coming back, and
   choosing any profile ends it. Nothing is changed without the choice.

What was left out of this decision, and why:

- A CPU quota (`cpu_max`) for a domain. The weight shares the CPUs while
  they are busy, which is what a service needs, and a quota needs the
  scheduler to stop a runnable thread.
- Rates for each disk, which the design's token buckets had. A domain's
  rates count across all disks together, so that a service cannot
  double its rate with a second disk and a bucket exists for each domain
  and not each domain and disk.
- A weight for the page cache's writeback. Its requests are the kernel's,
  so no domain's weight decides their share and no rate holds them; the
  rate holds the writer once its write has left pages dirty.
- More than 4,032 descriptors in one process. A capability space has room
  for them and no more, whatever `open_files` says.
- `cpu.idle`, as the idle loop never goes deeper than C1 yet, and a TCP
  buffer size, as a socket's buffers are fixed when it is made. Each
  waits for the kernel to have what it would set.

## Consequences

- A fork bomb or a runaway reader in a service stops at its own limit and
  the rest of the machine goes on answering, which the boot tests `limits`
  and `io-weight` and the lab tests `services/limits` and
  `storage/io-weight` show: a service held to 64 MiB is stopped at it
  while another runs, a service with `threads = 64` gets EAGAIN and a
  shell command answers within 2 seconds, weights of 100 and 400 give 3
  to 5 times the bytes in 10 seconds on a disk QEMU holds to 64 MiB a
  second, and `io_read = "10M"` gives 9 to 11 MiB a second.
- With no weight set the disk queue is as before, and a domain with no
  rate takes no bucket, so a machine that uses none of this pays for none
  of it.
- While a domain has a weight, each disk has 8 requests on it at most.
  That keeps a disk busy with large requests, but an NVMe disk's small
  random reads, which need more at once for their fastest, are slower
  until the weights are taken off.
- A kernel setting changed by `tune` does not outlast the boot. The
  `tune` service sets the profile again at each start, and the kernel
  logs each change with the process that made it.
- `tune` has no profile file of its own to keep apart from the choice:
  the choice is one line in the settings, which the snapshots keep, and
  the profiles come with the system.
- What Dukovo does better: a disk weight that holds whatever the disk's
  scheduler is, where systemd's `IOWeight=` needs one that honours it;
  `service status` showing a service's disk use and waits beside its
  memory and CPU, where Linux needs `systemd-cgtop`, `iotop` and cgroup
  files; and six short readable profiles, a switch that puts back
  everything the old one set, and `tune` marking which values the profile
  set.
- Later work, in `docs/deferred.md`: a CPU quota, rates for each disk, a
  weight for writeback, a queue depth for weights that follows the disk,
  more descriptors for one process, the idle depth and the TCP buffer
  size.
