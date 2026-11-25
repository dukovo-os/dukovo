<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0658. A file no one may write shares its pages, and refuses writers while it does

- Status: Accepted
- Date: 2026-11-25

## Context

ADR 0563 keeps one copy of a program's pages for every process that runs
it, but only for a file that cannot change: one in a read-only Btrfs
subvolume, as the system's slots are, or in the startup archive. Any
other file is mapped as before, each process filling its own copy of
each page the first time it touches it.

The catalog's bundles are installed into `/applications` or a person's
`~/applications` and sealed by their files' owner and mode (ADR 0223):
every file is root's and 0444, or 0555 if it may run. Read-only
subvolumes for sealed bundles are not done yet. So PostgreSQL's
`postgres` is a file that can change, and each backend, a fork of the
postmaster, fills its own copy of the parser, the planner and the
executor, which the postmaster never ran.

On OptiPlex 1 (6 MiB of L3 shared by four cores), PostgreSQL's select
and TPC-B rows took 1.08 and 1.22 times Linux's time. Dukovo ran 5 to 8%
more instructions, but 17 to 42% more cycles, with the same L2 misses
and 10 to 22 times Linux's L3 misses: eight backends' copies of the hot
code do not fit in the L3 where Linux's one copy does. A lab-only boot
flag that kept the pages of every file programs map brought the rows to
0.97 and 0.93 of Linux's time, with Linux's L3 misses.

Three ways would let a bundle's pages be kept:

- a read-only subvolume for each bundle, as ADR 0223 planned. Dukovo's
  Btrfs cannot rename a subvolume, which bundled does to put a version
  in place and to go back to the one before; a subvolume cannot be
  removed while a program runs from it; and both backup formats and
  machine restore leave out the subvolumes inside a place;
- a flag on each file that keeps it from changing, Linux's
  `FS_IMMUTABLE_FL`, kept on the disk. Linux's flag also refuses a
  removal, so bundled would clear it before every removal, as would
  everything else that removes a bundle's files, such as an account's
  removal with its home;
- a rule in the kernel: a file no one may write by its mode is kept from
  being written while its pages are kept. Linux refuses an open to
  write a program that runs with ETXTBSY, as Unix systems long have.

## Decision

- **A file no one may write by its mode has its pages kept.** A regular
  file with no write bit for its owner, its group or others, on a
  filesystem that counts its files' writers, has its pages kept as ADR
  0563 keeps a read-only file's. An access list gives no named user or
  group more than its mask, which is the mode's group bits, so only a
  caller past the second gate (ADR 0214), such as root, may still open
  it to write.
- **Writers and kept pages exclude each other.** The filesystem counts,
  for each file, the handles that may write it and the sets of pages
  kept of it, in one number (`radix_vfs::writes`): an open to write the
  file, or to cut it, takes a writer, and the kernel keeps the file's
  pages only while it has none. While pages are kept, the open fails
  with `text_busy`, POSIX's ETXTBSY, before it cuts anything; while a
  handle may write the file, each process fills its own copies, as
  before. The handle keeps its writer until the last capability to it
  goes. The pages kept go with the last region that maps the file (ADR
  0563), and writes may open again then.
- **Every change to a file's bytes goes through a writer.** A write, a
  change of size, a clone into the file and an open's cut all need a
  handle the open let write, and a shared writable mapping of a file is
  refused (ADR 0563). Removing, renaming and linking the file, and
  changing its owner, mode or times, change no bytes and take no
  writer, so bundled replaces, removes and goes back to versions as
  before.
- **Btrfs and tmpfs count their files' writers.** Their files change only
  through the kernel's writes. A filesystem a program serves, or one on
  another machine, counts none, and programs that map its files share
  none of their pages.

## Consequences

- PostgreSQL's backends, and the processes of every other bundle, share
  one copy of their program's code and read-only data, as the system's
  services do. Nothing changes on the disk, in bundled or in backups:
  a restored bundle is sealed by its mode as it was.
- Writing in place to a program or a library no one may write, while a
  program maps it, fails with ETXTBSY: `cp` over a running bundle's
  program by root, for one. Linux fails the same open for a program
  that runs, though not for a library, which a write in place would
  change under the programs that map it.
- A program that maps a file no one may write, readable to it, keeps the
  file's owner from writing it in place until it unmaps it. The owner
  must give themselves write permission first, and a file anyone may
  write is never kept.
- A file open to write when a program maps it is mapped as before, a
  copy for each process, until the program maps it again with no
  writer left.
