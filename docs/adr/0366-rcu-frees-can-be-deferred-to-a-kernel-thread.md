<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0366. RCU frees can be deferred to a kernel thread

- Status: Accepted
- Date: 2026-11-04

## Context

The tmpfs open+close bench row is at 0.94 of Linux on an EPYC 9V45
(#210). What is left of its cost is mostly locked instructions: 16 per
open and close, against Linux's 6 to 8. Three of them are the directory's
lookup: its mutex, taken and let go, and the found node's reference
count. Linux looks names up without a lock (RCU path walk), and frees a
removed entry only once no reader can still see it, with `call_rcu`.

Radix's RCU could only wait (`synchronize`). A grace period lasts until
every CPU has switched threads, up to a timer tick, so a writer that
waited for one on each unlink would make removing a directory of files
take milliseconds a file.

## Decision

- `radix_sync::rcu::defer` takes a `Deferred`, kept inside what it will
  free so that deferring needs no memory, and returns at once. The call
  runs once every read section running when it was deferred has ended.
- The kernel queues it, in its grace period state, with the grace period
  `synchronize` would wait for, and asks for that grace period. The `rcu`
  thread wakes when the first call's grace period completes and makes
  every call whose grace period has completed, in order.
- With one CPU there is no other read section: the call is made at once.
  Without the kernel's hooks (host tests), the hooks' default waits for a
  grace period and makes the call.
- `defer` must not be called inside a read section.

## Consequences

- Lock-free lookups can free what they reach without a writer waiting:
  tmpfs's are next.
- The `rcu` self-test also frees 1,000 objects by deferred calls while
  readers on three CPUs check them, and fails if any call runs early or
  never.
