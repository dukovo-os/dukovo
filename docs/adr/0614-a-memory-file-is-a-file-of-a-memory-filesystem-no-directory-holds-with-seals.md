<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0614. A memory file is a file of a memory filesystem no directory holds, with seals

- Status: Accepted
- Date: 2026-11-23

## Context

Linux's `memfd_create` gives a program a file in memory that no directory
holds: it is read, written, sized and mapped as any file, shared with
another process by passing the descriptor, and goes when the last
descriptor and mapping go. Seals let a receiver trust it: once sealed, the
file cannot shrink, grow, be written or be sealed further. Wayland
compositors, browsers, language runtimes and sandboxes use it to share
memory without a name another process could open. Radix's shared memory
filesystem on `/devices/shm` (ADR 0490) already keeps files whose pages
every mapping shares (`SharedPages`), but only in a directory a sandbox
must hold.

## Decision

- **A memory file is a file of a second filesystem of the `/devices/shm`
  kind** (`fs/shm`'s `memory_file`), which no directory of any tree
  holds: radix-process(2)'s `memory_file` (version 13) makes one in a slot,
  read-write, so a program whose sandbox has no `/devices/shm` has memory
  files all the same. Its pages are a `SharedPages`, as a shared
  anonymous mapping's are, so a shared mapping of it maps the very pages
  every other process maps; it is sized, read, written, mapped and
  charged as a file of `/devices/shm` (half of memory for all of them, as
  a tmpfs may hold). Its name, at most 249 bytes, appears only in
  radix-file(2)'s `path`, as Linux's `/memfd:NAME (deleted)`.
- **Seals** are radix-file(2)'s `seal` (version 14), whose bits are Linux's
  `F_SEAL_` bits: `seal`, `shrink`, `grow`, `write`, `future_write` and
  `exec`. The file's `SharedPages` keep them, so the paths that size,
  write and map it see them where the change is made: a write or a size
  change a seal forbids fails with not_permitted, a shared writable
  mapping of a file sealed with `write` or `future_write` is refused,
  adding `write` while the file is mapped shared and writable fails with
  busy, and nothing is added once it is sealed with `seal`. A file made
  without `allow_sealing` starts sealed with `seal`, and one made with
  `no_exec` starts sealed with `exec`, as Linux's `MFD_NOEXEC_SEAL`.
- **The Linux table** gives `memfd_create` and `fcntl`'s `F_ADD_SEALS` and
  `F_GET_SEALS`; the flags (`MFD_CLOEXEC`, `MFD_ALLOW_SEALING`,
  `MFD_NOEXEC_SEAL`, `MFD_EXEC`) and the seal bits are checked in
  `radix_linux::memfd`, with host tests. `MFD_HUGETLB` is EINVAL, as on a
  Linux without huge pages. Dukovo's C library's `memfd_create` and
  `fcntl` make the same calls.

## Consequences

- A memory file needs no capability but the process's own: anything that
  passes a descriptor can share memory, and a receiver checks the seals.
- The `posix` boot test runs `memfd.c` natively and under
  `linux-programs`: writing, mapping shared across `fork`, each seal's
  refusal, busy for `write` over a writable mapping, and `seal` itself.
- `mremap` of a memory file's mapping that would grow it is still ENOMEM
  (docs/deferred.md, for the seccomp and JIT step of Phase 18).
