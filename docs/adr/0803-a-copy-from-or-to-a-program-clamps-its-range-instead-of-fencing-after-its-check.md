<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0803. A copy from or to a program clamps its range instead of fencing after its check

- Status: Accepted
- Date: 2026-11-28

## Context

`copy_from_user` and `copy_to_user` check that a program's range lies
wholly in the lower half, below `USER_TOP`, before `radix_user_copy`
copies it. A processor may guess that check's outcome and copy before the
check is settled, from an address in the kernel's half that a program
chose, and leave what it read in the cache (Spectre variant 1). Since
ADR 0790 the copy ran LFENCE after its check, which stops that: no
instruction after the fence starts until every one before it has
finished.

That wait is on every system call that copies, which is most of them:
`poll` copies its entries in and the events out, `read` and `write`
their data, every call that takes a structure. The processor cannot
overlap the copy with the call's entry, the method's lookups or
anything else still in flight when the copy starts. Linux replaced the
same fence with address masking in 2024 (`mask_user_address`), which
leaves the processor free to run ahead and makes what it runs ahead with
harmless.

## Decision

- **`radix_sync::nospec::range` clamps a range.** It gives `address`
  and `len` when the `len` bytes from `address` end at or below `top`,
  and `top` and 0 when they do not: their end past `top`, or past the
  top of the address space. It computes a mask from the carry of the
  addition and the borrow of the comparison, with `sbb` and no branch,
  so there is no outcome for the processor to guess. A host test checks
  it, and the same mask in plain arithmetic, against the range check.
- **`copy_from_user` and `copy_to_user` copy the range it gives**, after
  their check, and `radix_user_copy` no longer fences. For a range that
  passed the check the clamp changes nothing. A processor that runs past
  a failed check copies 0 bytes from `USER_TOP`, the start of the page
  at the top of the lower half that is never mapped, so it reads nothing
  of the kernel's.
- `nospectre_v1` and `mitigations=off` leave the clamp in place, as
  they leave `radix_sync::nospec::index`: it costs a few instructions
  that do not wait, and a second copy routine for the off case would cost
  more than it saves. The report under `spectre_v1` stays Linux's
  "Mitigation: usercopy/swapgs barriers and __user pointer
  sanitization".

## Alternatives

- **Keep the fence.** It is the simplest form and was correct, but it
  holds back every copying call for what a few instructions without a
  wait do as well.
- **Mask only the address, as Linux's `mask_user_address` does**, so
  that one past the top becomes an address that faults, and rely on a
  range that starts below the top and runs past it faulting at the
  unmapped page and the hole above the lower half before it reads
  anything. It needs one instruction fewer. Clamping the length too makes
  the copy a processor guesses empty, so the argument does not depend on
  how far a processor runs ahead through a fault.
- **The interrupt entry's fence after SWAPGS** stays (ADR 0791): that
  choice is between two values of GS, not a range, and the fence is
  patched out where it is not needed.

## Consequences

- A system call that copies no longer waits at the copy for everything
  before it to finish. On optiplex-1 the change measured within the
  runs' spread (a `poll`'s kernel cycles 3258 before, 3252 to 3276
  after): there the entry and exit switch page tables and write
  IA32_SPEC_CTRL, and the fence's wait overlapped with theirs. Where the
  entry and exit are lighter (enhanced IBRS, no page-table switch) the
  fence is a larger share of a call; that was not measured.
- ADR 0790's Spectre v1 point holds with "copies from programs clamp
  their range after their access check" in place of the fence.
- A new routine that copies from or to a program's memory must pass its
  range through `radix_sync::nospec::range` after its check, as
  docs/coding-standard.md says of indices and `radix_sync::nospec::index`.
