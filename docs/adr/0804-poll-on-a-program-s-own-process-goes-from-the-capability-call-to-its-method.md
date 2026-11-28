<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0804. `poll` on a program's own process goes from the capability call to its method

- Status: Accepted
- Date: 2026-11-28

## Context

A method of a process capability goes from the capability call
(`methods::call`) to the process interface (`process::method`), which
checks the capability's rights, finds the process and matches the
label, and for a POSIX method from `fork` on to `posix::method`, which
checks that the process is the caller's own and matches the label again.
Each of the two is kept out of line, as `methods::call` keeps every
kind's interface, so that the frames under the deep methods stay small,
and each has a frame of its own: seven registers saved and restored, and
a search of the labels.

On optiplex-1 (Core i5-6500), after ADRs 0802 and 0803, a `poll` of one
ready pipe still took 1045 to 1047 ns against Linux's 1042 to 1044. Its
counters split the round:

| Cycles of one `poll` | Dukovo | Linux |
| --- | --- | --- |
| A system call neither system knows, in the kernel | 2784 | 2827 |
| `poll` of no descriptors, in the kernel, past that | 279 | 226 |
| Each descriptor, in the kernel | 198 | 224 |
| The program and the C library | 79 | 54 |

Dukovo's entry and exit, and its look at each descriptor, are faster
than Linux's. Its fixed part of a `poll` ran about 610 instructions past
a bare call, against Linux's 310, and the kernel's instructions on this
path cost about half a cycle each.

`poll` is the call an event loop makes most: `poll`, `ppoll` and
`select` all make it, and programs that wait on descriptors make it once
a wake-up or more.

## Decision

- **`methods::call` sends `poll` on a process capability to
  `process::poll`**, past `process::method` and `posix::method`. It finds
  the process with `process::method`'s own lookup (`target`: the
  capability's CONTROL right, the process it designates or the caller's
  own) and then calls `posix::poll_method`.
- **`posix::poll_method` is the one `poll`.** It checks that the process
  is the caller's own, as `posix::method` does for all its methods but
  `exec` and `start_limit`, decodes the arguments, polls and answers.
  `posix::method`'s `POLL` arm calls it too, so both routes give the
  same errors in the same order.
- The other methods of the process interface keep their route: none of
  them is made often enough for its two frames to show.

## Alternatives

- **Every POSIX method on the caller's own process from the capability
  call**, as one route past `process::method`. It saves one frame of
  the two, and `posix::method`'s match stays; for `poll` that is about
  half of what this route saves.
- **The two interfaces inlined into `methods::call`.** Their frames
  would become `methods::call`'s, under every method of every kind, and
  the deep ones (a mount's, an `exec`) would carry them.
- **A system call number of its own for `poll`**, as Linux has one for
  each call. It would bypass the capability lookup too, but the process
  capability is what gives a program its POSIX calls, and every other
  method would still take the long route.

## Consequences

- A `poll` runs about 50 kernel instructions fewer, two frames and two
  label searches. On optiplex-1, TODO-NUMBERS.
- A new method that takes this route needs a reason as `poll` has, and
  must go through `target` and the own-process check as `poll` does.
- Every `poll`, `ppoll` and `select` takes the route, so the boot tests
  `posix/poll` and `posix/cancel` and every program that waits on
  descriptors run it; its checks are `process::method`'s and
  `posix::method`'s own functions and conditions, in their order.
