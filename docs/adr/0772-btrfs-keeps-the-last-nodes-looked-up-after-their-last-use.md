<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0772. Btrfs keeps the last nodes looked up after their last use

- Status: Accepted
- Date: 2026-11-26

## Context

A Btrfs directory keeps the names its lookups found, with the nodes they
led to held weakly (ADR 0659), and the table of nodes in use holds them
weakly too (`Inner::nodes`), so a node goes when its last holder lets go
of it. Between two requests PHP-FPM holds neither its script's node nor
those of the directories on its path: the VFS's walk holds each only
while it looks in it. So each request found its names kept but their
nodes gone, and read the inodes, and the directories' entries before
them, from the trees again. After ADR 0659, the OptiPlex's profile of
PHP-FPM behind nginx still put about as many samples in `node_for` and
`read_block` as before it (183 and 164, against 209 and 157), and the
row stayed at 1.08 times Linux's time per request.

Linux keeps an inode, and the dentry that names it, after its last use,
until memory runs short.

## Decision

- **The filesystem keeps the last 256 nodes looked up** (`Inner::kept`):
  `node_for` puts each node it returns in a ring, unless the node says
  it is there already (a flag read without a lock). Once the ring is
  full each new node replaces the next in turn, the oldest or near it,
  which goes after the ring's lock, since that may be its last
  reference. A kept node is in use as any other: the names kept for
  it lead to it, and memory pressure takes its page cache
  (`FileSystem::shrink` walks the nodes in use).
- **A node only kept counts as not in use where a change asks.** `live`,
  which finds the node a change to an inode must update, and by which
  removal decides whether a file goes with its last name or waits for
  its last close, lets go of the node first, and answers none if nothing
  else held it; the next lookup reads it again. Removing a subvolume
  lets go of its kept nodes before it asks whether any is in use. So a
  closed file's items and room still go with its last name, as on
  Linux, and a subvolume whose files were only looked up is removed.
- **They go with the filesystem.** Each node holds what the
  filesystem's nodes share, the ring with them, so the filesystem lets
  go of the ring as it goes, and keeps nothing after.

## Alternatives

- **Names that hold their nodes strongly.** A directory would hold every
  child it named while it is held, and a kept directory its whole
  subtree, with no bound but memory pressure, which would then have to
  find them and let go.
- **Every node kept until memory runs short, oldest dropped first, as
  Linux does.** It needs a list in order of use and a shrinker that
  walks it; the bound does without both, and a node's pages, most of
  what it holds, already go back under pressure while it is kept.
- **A cache of the paths walked, in the VFS.** Radix's VFS asks each
  directory for each name and holds nothing between walks; a cache
  there would serve every filesystem, but would have to learn each
  one's rules for when a name stops leading where it did, which Btrfs's
  counts already follow (ADR 0659).

## Consequences

- A name looked up again leads to its node with no walk of the trees,
  however long ago its last holder let go of it, while 256 other nodes
  have not been looked up since.
- Up to 256 nodes stay after their last use, a few hundred bytes each,
  with a directory's kept names (ADR 0659) and a file's cached pages,
  which memory pressure takes back as from any node in use.
- Every lookup that reaches `node_for` reads one more flag, and a node
  newly kept takes the ring's lock.
- The unit tests check that a file no one holds is found again while
  another thread holds the transaction, that the ring keeps the last
  256 nodes, that the kept nodes go with the filesystem, and that a
  subvolume whose files were looked up is removed; the tests of removal
  check that a closed file's items and room still go with its last
  name.
