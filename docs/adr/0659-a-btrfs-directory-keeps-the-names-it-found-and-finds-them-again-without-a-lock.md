<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0659. A Btrfs directory keeps the names it found, and finds them again without a lock

- Status: Accepted
- Date: 2026-11-26

## Context

A lookup in a Btrfs directory on a writable mount holds the
filesystem's transaction while it walks the directory's tree to the
name's entry (a change edits the tree's blocks in place, ADR 0129), then
takes the table of nodes in use to find or add the node (`node_for`).
Each lookup took those two locks, shared by every CPU, and the walk
under the first. Linux's path walk finds a name in the dentry cache in
RCU mode with neither, and tmpfs here already does the same (ADR 0367).

Programs open files by their full paths, so the same few directories
are looked up over and over: PHP-FPM opens its script and the files
PHP includes on every request, nginx opens what it serves, and both
look up `/tmp`. A profile of PHP-FPM behind nginx on the OptiPlex
(1.10 times Linux's time per request) put some 5 % of the kernel's
time per request in Btrfs lookups (reading tree blocks, `node_for`,
the walk), besides a share of the time threads slept for a mutex, where
Linux's profile had none of it.

## Decision

- **A directory keeps the names its lookups found** (`names::Names`):
  an `RcuMap` (ADR 0367) of each name to the node it led to, held
  weakly, and the count of changes to the directory's entries as that
  lookup began. A lookup reads the count, then the map with no lock; a
  name kept with the same count, whose node is still in use, is the
  answer. Otherwise it walks the tree as before and keeps what it found.
- **The count moves after every change to a directory's entries.**
  `Writer`'s inserts, updates, deletes and resizes of a `DIR_ITEM` move
  the count of the directory the item belongs to (its tree and the
  key's object number), after the edit and before the transaction's
  lock goes. A lookup that kept a name read the count before its walk,
  and the walk held the transaction, so a change the walk did not see
  moved the count past what it read, and the name is not found again.
  A lookup racing a change finds the name as one of the two states had
  it, as tmpfs's do. Edits to the items are the one place every change
  passes through: creating, removing, linking, renaming, receiving and
  replaying a log all edit `DIR_ITEM`s there.
- **64 counts serve the whole volume** (`Volume::names_seen`): a
  directory uses the word its tree and inode number pick by Fibonacci
  hashing, so a subvolume's directories, numbered in a row, spread over
  all of them. A change in one directory sends the lookups of the
  directories sharing its word to the tree once more. A subvolume
  removed moves every word: its trees go with their blocks, not edit by
  edit.
- **A name is kept only for an inode of the directory's subvolume.**
  An entry that leads into another subvolume, and the empty directory
  that stands for a subvolume a snapshot did not take, are looked up
  each time, as before.
- **A directory keeps at most 64 names.** One more empties its map and
  it starts again, so a directory of many files costs at most 64 links
  and a table, and the common case, a few directories on every path,
  stays found. A node nobody holds goes as before; its name is then
  looked up in the tree again.

## Alternatives

- **One count for the volume.** Simpler, but a file made or removed
  anywhere, a PHP session or a temporary file, would send every
  directory's lookups back to the tree.
- **Moving a directory's own count in `create`, `remove`, `rename` and
  `link`.** Exact, but each path that changes entries (receiving a
  stream, replaying a log, removing a subvolume, the next one written)
  would have to remember to; a change that forgot would leave a name
  pointing where it no longer leads.
- **A cache of names for the whole filesystem, as Linux's dentry
  cache.** It would also keep names that were not found, and the
  directories' parents, which `..` and the VFS's path walk would use.
  Radix's VFS asks each directory for each name, so a directory's own
  map is where the lookup already is; negative entries are left for
  when a profile shows them.
- **Reading the tree without the transaction**, with the blocks a
  change replaces freed after a grace period. It would make every
  reader lock-free, files' extents included, but every edit of a block
  in place would need to copy it first: a far larger change to the
  copy-on-write tree for what a cache gives the common case.

## Consequences

- A lookup of a name found before takes no lock and writes nothing
  another CPU reads, as on tmpfs. The unit tests check that a kept name
  is found while another thread holds the transaction, that no rename,
  removal, link or replacement leaves a name where it was, and that
  lookups racing creates, renames and removals in the same directory
  find only what is there.
- Each directory node in use holds a map of up to 64 names, a few
  hundred bytes to a few KiB, and the volume 512 bytes of counts.
- The first lookup of each name after a change to its directory, or to
  one sharing its word, walks the tree and allocates a link.
