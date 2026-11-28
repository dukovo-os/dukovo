<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0802. A cancellation point's call passes its arguments and results in registers

- Status: Accepted
- Date: 2026-11-28

## Context

`posix_poll`, a `poll` of one pipe with data to read, was the one row of
the lab's ipc test over Linux's on optiplex-1 (Core i5-6500): 1053 ns
against 1042. Rows of counters on both systems split the round:

| Per call | Dukovo | Linux |
| --- | --- | --- |
| A system call neither system knows | 886 ns | 898 ns |
| `poll`'s cycles in the kernel | 3257 | 3277 |
| `poll`'s cycles in the program and the C library | about 115 | about 55 |
| `poll`'s instructions in the program and the C library | about 150 | about 40 |

The kernel's part, entry and exit with their protections (ADR 0790)
and the method itself, was already as fast as Linux's. The difference
was in the C library, on the way to the `syscall` instruction and back.

`poll`, like `read`, `write`, `accept` and the waits, is a cancellation
point, whose call goes through `radix::cancellable` or
`radix::cancellable_masked` (ADR 0104). A method's `_with` function put
its ten arguments in an array on the stack and called
`radix::cancellable_masked`, a function in radix.cpp, which called the
stub in cancel.S. The stub saved five registers, loaded the ten
arguments from the array, made the call and stored the ten results in
a second array, which the `_with` function then read. The C library's
other calls go through `radix_syscall`, which inlines, so that the
compiler keeps their arguments and results in registers. `poll_ns`, in
poll.cpp, also kept a frame and spilled registers for the copy of the
entries that only a `poll` of events the kernel does not know needs.

## Decision

- **`radix::cancellable` and `radix::cancellable_masked` are inline**,
  in radix.hpp, and keep `radix_syscall_fn`'s type. Each puts the call
  in its registers, as `radix_syscall` does, and calls its stub from
  inline assembly. The stack pointer moves past the 128-byte red zone
  first, since the return address would overwrite what a function
  without calls keeps there. With a method's `_with` function inlined
  too, the call's arguments and results stay in registers.
- **The stubs take the call as the `syscall` instruction does** and
  return as it does: the number in RAX, the arguments and the results in
  RDI, RSI, RDX, R10, R8, R9 and R12 to R15, RCX and R11 changed, every
  other register kept. Each reads the cancel bits and makes the call,
  and needs no memory.
- **A masked point has a stub of its own**,
  `__radix_cancellable_syscall_masked`, with its own begin, end and now
  labels, since a register that said which kind of call it was would
  have to be saved by every caller. SIGCANCEL's handler sends a thread
  between a stub's labels to that stub's now label. The unmasked one's
  aligns the stack and ends the thread; the masked one's returns
  CANCELED. The look at the cancel bits after a call that a signal
  interrupted stays in C, out of line (`radix::after_interrupt`).
- **`poll`'s common path has no copy and no frame for one.** When every
  entry asks only for events the kernel knows, `poll_ns` makes the call
  on the program's array. The copy for the band events is its own
  function, `poll_copy`, as is the path of a call that failed.
- **`poll` itself is the Radix layer's**, in poll.cpp, with `poll_ns`
  inline, as `ppoll` and `pselect` already were. mlibc's `poll` called
  the `Poll` sysdep, which returned an errno and the count through
  memory and was a second function between `poll` and the call. mlibc
  patch 0144 adds `MLIBC_SYSTEM_POLL` to its internal configuration,
  which a system layer sets to have `poll` itself, and leaves mlibc's
  out when it is set; the Radix layer sets it, and the `Poll` sysdep is
  gone.
- A call that takes the address of `radix::cancellable`, to choose
  between it and `radix_syscall` at run time, still works: the compiler
  keeps a copy of the function, whose arguments go through memory as
  before.

## Alternatives

- **The cancel check and the `syscall` instruction inline at each
  point**, with a table of every point's labels for the handler. It
  saves a call and a return a point, about two cycles, and makes the
  handler search a table the linker gathers from every object, which
  `--gc-sections` must be kept from dropping.
- **A kernel that keeps R12 to R15 when a call does not use them for its
  results**, which would spare the saves and restores of those four
  registers around many calls. It changes the contract of every system
  call for a few cycles, and is not needed for parity.
- **The kernel's part.** It was already faster than Linux's for `poll`;
  two changes to it that removed 50 instructions under QEMU measured no
  difference on optiplex-1.

## Consequences

- A `poll` of one ready descriptor runs 63 instructions in the C
  library, from about 145, and none of them touch memory for the call's
  arguments or results. `read`, `write`, `recv`, `send`, `accept`,
  `connect` and the waits make their calls as the C library's own plain
  calls do.
- Patch 0144 is one more patch to carry when mlibc is updated. It adds a
  configuration value and an `#if` around one function, and leaves
  mlibc's `poll` for every other system layer.
- On optiplex-1, the C library's part of a `poll` fell from about 115
  cycles to 86, and to 79 once `poll` was the Radix layer's own (Linux's
  is 54); `posix_poll` went from 1054 ns to 1045 to 1047, against
  Linux's 1042 to 1044. ADR 0804 closed the rest.
- `posix/cancel` checks both stubs: a cancel that comes while each
  point waits, one due before the call, a point that a handler with
  SA_RESTART makes again, and cleanup handlers; `posix/poll` checks the
  band events that take the copy.
