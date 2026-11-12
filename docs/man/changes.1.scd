changes(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

changes - what changed between two starts of the computer

# SYNOPSIS

*changes* [*--failed* | *--between* _start_ _start_ | *--list*]

# DESCRIPTION

changes says what changed since the last good start (ADR 0500): the
version of Dukovo, the kernel, the firmware set and its CPU microcode,
the boot options, the network cards and the root's partitions, and the
apps installed for everyone. Each start of the computer has a record,
which the health check begins as it runs and finishes with how the
start ended: *good*, when it was seen through or an update proved
itself; *failed*, with what failed and its error ID; or *unfinished*,
when the health check never finished.

Each change says when what is new was installed, where that is known,
and when a start first used it. A change installed days before it was
first used points at the restart that brought it in:

```
$ changes --failed
Start 42 on 2026-11-11 21:04, Dukovo 0.15.2, slot B: failed: the CPU microcode of firmware 2026.11.4 has data that failed its checks, so none of it was used (FW-101)
From the last good start, 41 on 2026-11-11 08:02, Dukovo 0.15.2, to start 42:
  firmware set: 2026.10.1 to 2026.11.4; installed 2026-11-04 10:12; first used 2026-11-11 21:04 (start 42)
  CPU microcode: 3f2a9c1d0b7e6a55 to 81c0e2d4f9a3b716; first used 2026-11-11 21:04 (start 42)
```

With no option, changes compares this start with the last good start
before it. The error ID leads to a page of the support center that
says what it means and what to do.

# OPTIONS

*--failed*
	Compare the newest start that failed with the last good start
	before it.

*--between* _start_ _start_
	Compare two starts, by number.

*--list*
	List the starts kept, newest first, each with how it ended.

# FILES

_/system/state/starts/_
	The record of each start, the last 100, by number.

# SEE ALSO

*sysupdate*(8), *health.toml*(5), *log*(1)
