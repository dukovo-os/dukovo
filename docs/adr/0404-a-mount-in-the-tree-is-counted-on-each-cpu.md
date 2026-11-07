<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0404. A mount in the tree is counted on each CPU

- Status: Accepted
- Date: 2026-11-07

## Context

Every place on a mount holds a reference to it (`MountRef`): each open
file, each process's root and working directory, each step of a walk. A
mount cannot be unmounted while one is held, and `unmount` checks that
with the reference count of the mount's `Arc`.

Each open clones that `Arc` and each close drops it, so every program
that opens files on the same mount, on any CPU, increments and
decrements one count on one cache line. With four processes opening and
closing files in `/tmp` on four CPUs (`ob bench --scaling`), that line
moved between CPUs 1.89 times for each open and close, the most of
anything the processes still shared once their accounts and capability
spaces had lines of their own, and each move costs the CPU that waits
for it about a hundred nanoseconds. Four processes took 1.57 times
Linux's time (the plan for making every scaling row beat Linux,
2026-11-06).

Linux keeps a count for each mount on each CPU (`mnt_pcp`): a reference
taken and dropped while the mount is in a namespace changes only the
count of the CPU it runs on, and only an unmount, or the last reference
after one, adds them up.

## Decision

- A mount in the tree has a count on each CPU, its own line in a table
  of 64 mounts' counts for each CPU. A reference to it made while it is
  in the tree adds one to the count of the CPU that makes it, and takes
  one from the count of the CPU that drops it, which may be another.
  Each reference remembers which way it was counted.
- Both are done inside an RCU read section that first sees the mount is
  in the tree. Every other reference is counted by the mount's `Arc`, as
  before: those of mounts that are in no tree (views'), of a mount past
  the 64th, and those made while an unmount looks at the mount. The
  mount of pipes and sockets, which is in no tree but is never
  unmounted and is held by every pipe and socket, is counted on each
  CPU for good.
- The counts on each CPU together hold one reference of the `Arc`, which
  they drop once the mount has left the tree and they add up to none.
  Until then no reference counted on the CPUs can outlive the mount.
- To see whether a mount is in use, `unmount` (and `unmount_tree`) first
  stops counting on the CPUs, then waits for every read section that
  might still change a count (`rcu::synchronize`). A reference made or
  dropped after that changes a count under a lock. The CPUs' counts,
  added up, and the `Arc`'s count then say exactly how many references
  there are. A mount still in use goes back to being counted on the
  CPUs; one that is not leaves the tree.
- Once a mount has left the tree, each reference counted on the CPUs
  that is dropped takes its one under the lock and adds the counts up.
  The one that brings them to none frees the mount's line, and drops
  the `Arc` reference the counts held.
- A tree that is dropped (`Vfs`), as tests drop theirs, takes its
  mounts out of counting on the CPUs in the same way.
- The counting is a reference type of the lock library,
  `radix_sync::cpu_ref::CpuRef`, Linux's `percpu_ref` in kind: it keeps
  the `unsafe` code, a copy of the `Arc` that the `Arc` does not count,
  out of the VFS, which is a compartment and has none.
- A TLA+ model, `kernel/fs/vfs/tla/MountRefs.tla`, checks that no
  reference outlives its mount, that `unmount` sees every reference, and
  that a line is clean when the next mount takes it. Without the grace
  period it finds an unmount that misses a reference.

## Consequences

- An open and close on a mount in the tree change only their own CPUs'
  counts: no line moves between processes on other CPUs for the mount.
  A reference costs a few loads and an atomic add on a line of the
  CPU's own, with interrupts held off around them: no timer tick,
  switch or idle comes between, so no grace period passes, and that is
  the read section. The kernel defines the count (`radix_count_here`),
  which `radix_sync` declares and fat LTO inlines into each clone and
  drop. A read section through the locks' hooks costs three calls
  through pointers, and letting preemption back a check for a
  reschedule, which letting interrupts back needs not. Counted under
  `ob bench --instructions`, a clone or drop costs some 20 instructions
  more than an `Arc`'s this way, where a call to the kernel's count
  through a pointer cost some 50: an open and close took 4,006
  instructions that way, over its budget of 3,950, and take 3,943.
- `stat` takes no reference to the mount at all where its walk stays on
  the one it starts from (`Vfs::look_at`): it only reads the node, and
  takes 1,822 instructions where it took 1,895 before.
- `unmount` waits for a grace period, up to a timer tick, as Linux's
  does (`synchronize_rcu` in `namespace_unlock`).
- A reference dropped after its mount left the tree adds up the counts
  of every CPU. Few are: a mount leaves only once nothing but the table
  and the unmount hold it.
- The table of counts takes 64 KiB: 256 CPUs, 64 mounts, four bytes
  each. A count may wrap, since one CPU may make references that others
  drop; the counts are added up wrapping, and their sum, the references
  there are, is always small.
- The locks' hooks (`radix_sync::Hooks`) say which CPU runs the caller,
  for the slow way and for the host's tests, as the lock library has no
  other way to know.
- A reference is two words instead of one: the `Arc`, and how it was
  counted.
