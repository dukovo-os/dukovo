<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0367. tmpfs looks names up without the directory's lock

- Status: Accepted
- Date: 2026-11-04

## Context

A tmpfs lookup took its directory's mutex, searched the index sorted by
name, cloned the node and let go: three locked instructions of the 16
that the open+close bench row pays (0.94 of Linux on an EPYC 9V45,
#210). Linux's path walk looks a name up in RCU mode, with no lock and no
reference until the end. ADR 0366 gave Radix's RCU deferred frees, which
a lock-free structure needs to free what a reader may still be looking
at.

## Decision

- `radix_sync::rcu_map::RcuMap` maps byte strings to values. It is a
  table of hash chains (FxHash) that readers walk in an RCU read section
  with no lock. Writers take the map's own mutex and publish a link with
  one store. A link taken out goes to a deferred call, which frees it
  once no reader can see it.
- A table grows to twice the chains once it holds more names than
  chains. The new table holds copies of the links and is published
  whole. The old one, which no writer changes again, goes to a deferred
  call with its links. A reader walking the old table still finds every
  name it held. Putting a name in cannot fail: its link is made first
  (`pending`), so the directory's entries and the map change together.
  A table without memory to grow stays as it is.
- Every tmpfs directory keeps such a map of its names beside its entries
  and sorted index. It changes them together, under its lock, in
  `add_name`, `remove`, `rename` and `rename_within`. `lookup` reads the
  map alone.

## Consequences

- Open+close does two locked instructions fewer (16 to 14), and a stat
  by name does too.
- Listings, and every change to a directory, still take its lock.
- Each name is kept twice, in the entries and in the map.
- A lookup that races a change finds the name as one of the two states
  had it.
