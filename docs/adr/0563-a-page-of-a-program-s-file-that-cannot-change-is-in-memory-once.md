<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0563. A page of a program's file that cannot change is in memory once

- Status: Accepted
- Date: 2026-11-15

## Context

`exec` and `mmap` of a file map its pages as the process's own copy,
filled when the process first touches each one (ADR 0060): a page from
the frame allocator, read from the file and owned by the space. Only a
fork shares pages, with the child. Every process that starts a program
afresh holds its own copy of each page of the program it runs.

Most of the system's services are one program, `systools`, whose
loaded parts are 6.5 MiB of read-only data and 26 MiB of code. On an installed
system at rest, 33 processes held 67 MiB of memory, and 58 MiB of it
were pages read from their programs' files, most of them copies of the
same pages of `systools`. The idle machine used 99 MiB of the 100 MiB
it may (ADR 0427), and any service that touched a little more of its
program put it over.

Linux maps the page cache's own pages: a file's page is in memory once
however many processes map it, until a process writes to its private
mapping and gets a copy. Radix's page cache cannot be mapped yet, which
is why a shared writable mapping of a file is refused.

The system's programs cannot change. They are in the running slot,
`@system.a` or `@system.b`, which are read-only Btrfs subvolumes (ADR
0451), or in the startup archive, and the filesystem refuses every
write to them.

## Decision

- **A file that cannot change keeps the pages filled from it.** When a
  space maps a file whose node says it cannot change (a file of a
  read-only subvolume, a received subvolume, or a read-only filesystem
  such as the startup archive), the mapping holds the file's
  `ProgramPages`: one per file, found by its node, made by the first
  space to map the file and gone with the last region that maps it.
- **A fault takes the kept page.** A fault on a whole page of such a
  file, in a region the space may not write (read-only or runnable),
  maps the page kept for that page of the file, counted as shared once
  more; with none kept, the space makes the page as before and keeps
  it, unless another space kept one meanwhile, which it then maps
  instead. The page's share count counts the spaces that map it and one
  more for the `ProgramPages`. Whether the file can change is asked
  again at each fault, and a file once found to be able to change has
  no page kept for any space from then on. The last page of a file, which is part file and
  part zeros, and pages of writable regions stay each space's own.
- **A kept page is never written in place.** Being shared, it is mapped
  read-only. A space that makes it writable with `mprotect` copies it at
  its first write, as after a fork, and the kernel's writes to a new
  program, its relocations, copy it first. So nothing a process does
  reaches another's pages. A kept page that no space maps once the
  copy is made is freed then, so it never stays beside its copy.
- **Each space is charged for it.** A space's domain is charged for a
  kept page while the space maps it, as for a page of its own, so a
  domain's limit means what it meant.
- **A page no space maps goes with a region of its file.** When a region
  of the file goes (`munmap`, a process's end, `exec`), the pages kept
  in the part of the file it spanned that no space maps any more are
  freed. A program's end frees the pages only it used; the last
  region's end frees the rest. Kept pages are never more than the
  copies they replace.
- **A page whose memory went bad is handed out no more.** A kept page
  the frame allocator retires (ADR 0168) is let go of at the next fault
  that would take it, and a new one is made. A space that meets the bad
  memory has its entry poisoned, as before, and the kept page is freed
  once no space maps it.
- **The list of files is short and taken only at mapping time.** The
  files are found by the address of their node in a sorted list under a
  lock, which only `exec` and `mmap` take. Each file's pages are in
  chunks of 512 under a lock of their own, which a fault takes once,
  and whose chunks are allocated without sleeping, so a fault never
  waits for memory with it held: where memory is that short, the page
  is the space's own.

## Consequences

- A program's code and read-only data are in memory once, however many
  processes run it.
- Pages that more than one holder shares are never swapped out, and
  kept pages are now shared pages. A service's code that it no longer
  runs stays in memory while the service runs, where before it could be
  swapped out; dropping cold kept pages under memory pressure, to read
  them again when touched, is left for later
  ([deferred work](../deferred.md)).
- A machine check on a kept page reaches every process that reads it
  afterwards, where before only the process whose copy it was met it;
  each gets SIGBUS with `BUS_MCEERR_AR` as before, and a service's
  supervisor starts it again with a sound page. A kept page is clean,
  so the page could instead be taken from every space and read again
  from its file, as Linux does with a clean page of its page cache; that
  is left for later ([deferred work](../deferred.md)).
- A file that can change, such as a program in a writable subvolume or a
  user's own build, is mapped as before: each process has its own copy.
- Once the page cache's pages can be mapped, kept pages give way to
  them, and shared writable mappings of files follow.
